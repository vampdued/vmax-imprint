//! VMAX brand system: Midnight dark only (the redesign handoff, SPEC §4,
//! says dark theme only — the Daylight theme and its switch were removed
//! 2026-11-17), colour tokens, radii and type.
//!
//! The brand kit is VMAX v1.1 (`~/workspace/user/files/vmax-brand-kit.zip`):
//! Black #000000, White #FFFFFF, Max Blue #2F5BFF, Graphite #17181B, Steel
//! #9AA1AD, blue-text-on-dark #8FA8FF. Chakra Petch for display (headings,
//! app title), IBM Plex Sans for everything people read, IBM Plex Mono
//! for code, log and JSON. The caption *rendering* fonts (`fonts.rs`,
//! DejaVu / cosmic-text) are untouched — this only styles the egui UI chrome.
//!
//! The handoff token set (SPEC §4) is the surface scale: bg #000, p1
//! #0b0c0e, p2 #17181B, p3 #202228, hairline ln #2b2e35, ok #4ADE80,
//! warn #FBBF24, err #F87171. `ln` is a hairline only (1.31:1 on card —
//! far below the 3:1 a control edge needs), so interactive controls get a
//! separate `field_border` derived from Steel; `ln` edges cards, gutters
//! and separators, never a button or a field.
//!
//! A calm desktop look: black/graphite surfaces, lightly rounded cards and
//! controls, sentence-case Plex throughout, and Max Blue — in small,
//! deliberate amounts — for highlights, progress, links and selection.
//!
//! Structure ported from `storytold/craft-anything`'s
//! `crates/ui-egui/src/theme.rs` (MIT OR Apache-2.0), adapted to stock egui
//! 0.37 and to this app; the palette and type are VMAX, not craft's.

use egui::epaint::text::VariationCoords;
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Stroke,
    TextStyle, Visuals,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeKind {
    #[default]
    Studio,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 1] = [ThemeKind::Studio];
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
    /// The icon glyph colour at rest (hovered icons use `text`).
    pub icon_rest: Color32,
    pub accent: Color32,
    /// Max Blue that reads as text on the theme's surfaces.
    pub accent_ink: Color32,
    /// Max Blue wash for soft highlights.
    pub accent_soft: Color32,
    pub separator: Color32,
    pub shadow: Color32,
    pub danger: Color32,
    pub warning: Color32,
    /// Success green (SPEC §4 `ok` #4ADE80): confidence chips, kept
    /// boxes, done states.
    pub ok: Color32,
    pub code_bg: Color32,
    pub radius_sm: f32,
    pub radius: f32,
    pub radius_lg: f32,
}

/// The app colour: Max Blue (#2F5BFF). White on it passes WCAG AA (5.2:1),
/// so it is the button fill; the lighter #8FA8FF is the small-blue-text
/// colour on dark surfaces, never a background behind text.
pub const BRAND: Color32 = Color32::from_rgb(0x2f, 0x5b, 0xff);

/// Max Blue's supporting colours, from the VMAX kit.
pub const BLUE_TEXT_ON_DARK: Color32 = Color32::from_rgb(0x8f, 0xa8, 0xff);
pub const STEEL: Color32 = Color32::from_rgb(0x9a, 0xa1, 0xad);

/// Redesign-handoff surface scale (SPEC §4): p1 #0b0c0e, p2 #17181B,
/// p3 #202228, hairline ln #2b2e35, ok #4ADE80, warn #FBBF24, err #F87171.
pub const P1: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x0e);
pub const P2: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);
pub const P3: Color32 = Color32::from_rgb(0x20, 0x22, 0x28);
pub const LN: Color32 = Color32::from_rgb(0x2b, 0x2e, 0x35);
pub const OK_GREEN: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
pub const WARN_AMBER: Color32 = Color32::from_rgb(0xfb, 0xbf, 0x24);
pub const ERR_RED: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            // VMAX midnight on the SPEC §4 surface scale: black chrome,
            // p1 canvas, p2 cards, p3 raised/hover, black fields (the
            // mockup's inputs sit on bg-black), `ln` hairlines. Text is
            // white, secondary text is Steel; the accent is Max Blue.
            ThemeKind::Studio => Tokens {
                kind,
                chrome: Color32::BLACK,
                canvas: P1,
                card: P2,
                card_border: LN,
                raised: P3,
                // A field reads as something to type in because it is
                // black on a p2 card, edged in `field_border` below.
                field: Color32::BLACK,
                // 3.62:1 on card, 4.28:1 on chrome (measured) — the SPEC
                // hairline `ln` is only 1.31:1 on card, so it must never
                // edge an interactive control.
                field_border: mix(STEEL, P2, 0.35),
                hover: P3,
                pressed: LN,
                text: Color32::WHITE,
                text_dim: STEEL,
                // Faint text (#8A919E, 5.75:1 on card, 6.22:1 on canvas, 6.67:1 on
                // chrome) passes WCAG AA across every surface including card rows,
                // inspector cards, and disabled controls.
                text_faint: mix(STEEL, P1, 0.10),
                icon_rest: STEEL,
                accent: BRAND,
                accent_ink: BLUE_TEXT_ON_DARK,
                accent_soft: Color32::from_rgba_unmultiplied(0x2f, 0x5b, 0xff, 44),
                separator: LN,
                shadow: Color32::from_black_alpha(120),
                danger: ERR_RED,
                warning: WARN_AMBER,
                ok: OK_GREEN,
                code_bg: Color32::BLACK,
                radius_sm: RADIUS_SM,
                radius: RADIUS,
                radius_lg: RADIUS_LG,
            },
        }
    }

    /// The tokens of the active theme (defaults to Studio dark before `apply` runs).
    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(tokens_id()))
            .unwrap_or_else(|| Tokens::for_kind(ThemeKind::Studio))
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

/// IBM Plex Sans, variable (wght 100–700): the brand body face — everything
/// people read. OFL, see `assets/fonts/IBMPlexSans-OFL.txt`.
pub static IBM_PLEX_SANS: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Variable.ttf");

/// The faces cut from it: text, labels, and headings and emphasis.
const PLEX_FACES: [(&str, f32); 3] = [
    ("IBMPlexSans", 400.0),
    ("IBMPlexSans-Medium", 500.0),
    ("IBMPlexSans-SemiBold", 600.0),
];

/// Chakra Petch, the brand display face: app title and headings only, never
/// body copy. Static SemiBold (+ Medium for the 500 weight, Bold for the
/// title bar). OFL, see `assets/fonts/ChakraPetch-OFL.txt`.
pub static CHAKRA_PETCH_SEMIBOLD: &[u8] =
    include_bytes!("../assets/fonts/ChakraPetch-SemiBold.ttf");
pub static CHAKRA_PETCH_MEDIUM: &[u8] = include_bytes!("../assets/fonts/ChakraPetch-Medium.ttf");
pub static CHAKRA_PETCH_BOLD: &[u8] = include_bytes!("../assets/fonts/ChakraPetch-Bold.ttf");

/// IBM Plex Mono (code, log, JSON, values). OFL, see
/// `assets/fonts/IBMPlexMono-OFL.txt`.
pub static IBM_PLEX_MONO: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// Install IBM Plex Sans (three weights: text, labels, and semibold for
/// headings), Chakra Petch (display) and IBM Plex Mono (code). The default
/// egui fonts stay behind them as fallbacks (emoji, CJK, and anything Plex
/// does not cover).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    for (name, weight) in PLEX_FACES {
        let tweak = FontTweak {
            coords: VariationCoords::new([(*b"wght", weight)]),
            ..Default::default()
        };
        fonts.font_data.insert(
            name.to_owned(),
            Arc::new(FontData::from_static(IBM_PLEX_SANS).tweak(tweak)),
        );
    }

    fonts.font_data.insert(
        "ChakraPetch".to_owned(),
        Arc::new(FontData::from_static(CHAKRA_PETCH_SEMIBOLD)),
    );
    fonts.font_data.insert(
        "ChakraPetch-Medium".to_owned(),
        Arc::new(FontData::from_static(CHAKRA_PETCH_MEDIUM)),
    );
    fonts.font_data.insert(
        "ChakraPetch-Bold".to_owned(),
        Arc::new(FontData::from_static(CHAKRA_PETCH_BOLD)),
    );

    fonts.font_data.insert(
        "IBMPlexMono".to_owned(),
        Arc::new(FontData::from_static(IBM_PLEX_MONO)),
    );

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "IBMPlexSans".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "IBMPlexMono".to_owned());

    let base: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (fam, stack) in weighted_families(&base) {
        fonts.families.insert(fam, stack);
    }

    ctx.set_fonts(fonts);
}

/// The medium and semibold families: Plex's own weight first, then the
/// whole proportional stack. `display` is Chakra Petch semibold first, then
/// the proportional stack (so a missing glyph falls back to Plex, never to
/// a random system font).
fn weighted_families(base: &[String]) -> Vec<(FontFamily, Vec<String>)> {
    let mut out: Vec<(FontFamily, Vec<String>)> = [
        ("medium", "IBMPlexSans-Medium"),
        ("semibold", "IBMPlexSans-SemiBold"),
    ]
    .into_iter()
    .map(|(fam, primary)| {
        let mut stack = vec![primary.to_owned()];
        stack.extend(base.iter().cloned());
        (FontFamily::Name(fam.into()), stack)
    })
    .collect();
    let mut display = vec!["ChakraPetch".to_owned()];
    display.extend(base.iter().cloned());
    out.push((FontFamily::Name("display".into()), display));
    out
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// The display face: Chakra Petch semibold at `size`.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

/// The type scale: every text size in the interface is one of these.
/// Headings (`heading`, Chakra Petch semibold), largest first: a screen's title, a step or a dialog's
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

/// Code, paths and numbers in IBM Plex Mono.
pub const CODE: f32 = 12.5;

/// The spacing steps: the gaps between things are built from these. (Panel and
/// window dimensions are layout literals in `ui.rs`, not scale steps.)
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
    pub const MENU: Margin = Margin::symmetric(8, 6);
    /// Code blocks and dense workbench boxes.
    pub const TIGHT: Margin = Margin::same(8);
    pub const PANEL: Margin = Margin::same(12);
    pub const CARD_PAD: Margin = Margin::same(16);
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
    pub const ALL: [ControlSize; 4] = [
        ControlSize::Small,
        ControlSize::Medium,
        ControlSize::Large,
        ControlSize::Hero,
    ];

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

/// A heading: Chakra Petch semibold, at one of the heading sizes above.
pub fn heading(text: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(text).font(display(size))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// `a` blended toward `b` by `t` (0..=1), channel by channel (alpha included).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        m(a.r(), b.r()),
        m(a.g(), b.g()),
        m(a.b(), b.b()),
        m(a.a(), b.a()),
    )
}

/// A short label (captions, buttons, the title bar): Plex medium at one of the text sizes.
pub fn label_text(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text).font(medium(size))
}

/// Scroll bars that can always be seen and grabbed. egui's thin preset paints the thumb in the
/// button fill and its track in the field colour, which here are the same, and narrows it to 2 px
/// at rest, so it showed only on hover. The thumb is the text colour, see-through enough to stay
/// quiet but 3:1 against the surfaces it sits on (WCAG 1.4.11), 6 px at rest and 8 px on hover,
/// with a non-floating 10 px allocated track so the thumb never collides with selection borders.
pub fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating: false,
        bar_width: 8.0,
        floating_width: 6.0,
        floating_allocated_width: 10.0,
        handle_min_length: space::XXL,
        foreground_color: true,
        // No track: only the thumb shows, a pill that thickens a little under the pointer.
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: SCROLL_THUMB_AT_REST,
        active_handle_opacity: 0.45,
        interact_handle_opacity: 0.65,
        ..egui::style::ScrollStyle::solid()
    }
}

/// How opaque a scroll bar's thumb is at rest (over the text colour).
const SCROLL_THUMB_AT_REST: f32 = 0.22;

/// Apply a theme. `roomy` = bigger type, more air (not used here; the app runs compact).
pub fn apply(ctx: &egui::Context, kind: ThemeKind, roomy: bool) {
    let t = Tokens::for_kind(kind);

    ctx.data_mut(|d| d.insert_temp(tokens_id(), t));

    ctx.set_theme(if t.dark() {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });

    let mut v = if t.dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };

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
    // Keep disabled controls legible (WCAG AA >= 4.5:1 instead of stock 0.5x alpha = 4.28:1).
    v.disabled_alpha = 0.65;
    v.hyperlink_color = t.accent_ink;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.danger;
    v.window_corner_radius = t.r_lg();
    v.menu_corner_radius = t.r();
    v.window_shadow = egui::Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: t.shadow,
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: t.shadow,
    };
    v.text_edit_bg_color = Some(t.field);
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.handle_shape = egui::style::HandleShape::Circle;
    v.striped = false;
    v.indent_has_left_vline = false;

    // egui's own buttons, combo boxes and fields share the corner of ours: 6px (`r`),
    // never 999px pills on `small_button()`.
    let r = t.r();
    let w = &mut v.widgets;

    w.noninteractive.bg_fill = t.card;
    w.noninteractive.weak_bg_fill = t.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_dim);
    w.noninteractive.corner_radius = r;

    // Buttons and combo boxes are field-coloured boxes with a hairline edge.
    // One edge for every control, ours and egui's: firm at rest (field_border
    // is 3.62:1 on card and 4.28:1 on chrome — above the 3:1 a non-text edge
    // needs; the SPEC hairline `ln` is only 1.31:1 on card, so it edges
    // cards and gutters, never a control), firmer under the pointer, the app
    // colour while it's pressed or open.
    for (wv, bg, weak, stroke) in [
        (&mut w.inactive, t.field, t.field, t.field_border),
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

    let control = if roomy {
        ControlSize::Medium
    } else {
        ControlSize::Small
    };

    ctx.global_style_mut(|s| {
        s.text_styles = [
            (
                TextStyle::Small,
                FontId::proportional(if roomy { LABEL } else { CAPTION }),
            ),
            (
                TextStyle::Body,
                FontId::proportional(if roomy { BODY } else { LABEL }),
            ),
            (TextStyle::Button, medium(control.text())),
            // egui panics resolving a missing style, so Heading stays as the
            // framework fallback — but the app's headings go through
            // `theme::heading()`, never `ui.heading()`, so this is the only
            // heading path in practice.
            (
                TextStyle::Heading,
                semibold(if roomy { SECTION } else { CARD }),
            ),
            (
                TextStyle::Monospace,
                FontId::monospace(if roomy { LABEL } else { CODE }),
            ),
        ]
        .into();

        s.spacing.item_spacing = egui::vec2(space::SM, if roomy { space::SM } else { space::XS });

        // Padding that brings a line of the label up to the control's height; a row of plain text
        // stays as tall as a small control (minimum 24px WCAG 2.2 AA target height).
        let pad_y = ((control.height() - control.text() * 1.2) / 2.0)
            .round()
            .max(4.0);
        s.spacing.button_padding = egui::vec2(if roomy { space::MD } else { 10.0 }, pad_y);
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
        let heading = ctx
            .global_style()
            .text_styles
            .get(&TextStyle::Heading)
            .cloned()
            .unwrap();
        assert_eq!(heading.family, FontFamily::Name("semibold".into()));
        // The display helper resolves to the Chakra Petch family.
        assert_eq!(
            crate::theme::display(18.0).family,
            FontFamily::Name("display".into())
        );
    }

    /// Relative luminance (WCAG 2.1) of an opaque colour.
    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let s = v as f32 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (l1, l2) = (luminance(a), luminance(b));
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Every text pair the UI actually uses must pass WCAG AA (4.5:1 for
    /// text, 3:1 for the focus ring and control edges). Measured values:
    /// text 19.57:1 on canvas, dim 7.52:1, faint (`mix(Steel, p1, 0.10)`)
    /// >= 5.75:1 on card, canvas, and chrome.
    #[test]
    fn vmax_contrast_pairs_pass_aa() {
        for kind in ThemeKind::ALL {
            let t = Tokens::for_kind(kind);
            // Body text and faint text on every surface they sit on.
            for (bg, name) in [
                (t.canvas, "canvas"),
                (t.card, "card"),
                (t.field, "field"),
                (t.chrome, "chrome"),
            ] {
                assert!(contrast(t.text, bg) >= 4.5, "{kind:?} text/{name}");
                assert!(contrast(t.text_dim, bg) >= 4.5, "{kind:?} dim/{name}");
                assert!(contrast(t.text_faint, bg) >= 4.5, "{kind:?} faint/{name}");
            }
            // Blue text on every surface it sits on, and white-on-accent.
            for (bg, name) in [(t.canvas, "canvas"), (t.card, "card"), (t.field, "field")] {
                assert!(
                    contrast(t.accent_ink, bg) >= 4.5,
                    "{kind:?} accent_ink/{name}"
                );
            }
            assert!(
                contrast(Color32::WHITE, t.accent) >= 4.5,
                "{kind:?} white/accent"
            );
            // Functional text.
            assert!(contrast(t.ok, t.canvas) >= 4.5, "{kind:?} ok/canvas");
            assert!(
                contrast(t.warning, t.canvas) >= 4.5,
                "{kind:?} warning/canvas"
            );
            assert!(
                contrast(t.danger, t.canvas) >= 4.5,
                "{kind:?} danger/canvas"
            );
            // The focus ring (accent_ink, 1.5px) on the surfaces it rings.
            for (bg, name) in [(t.field, "field"), (t.chrome, "chrome")] {
                assert!(contrast(t.accent_ink, bg) >= 3.0, "{kind:?} ring/{name}");
            }
            // Control edges (field_border) on the surfaces they edge.
            for (bg, name) in [(t.card, "card"), (t.chrome, "chrome")] {
                assert!(contrast(t.field_border, bg) >= 3.0, "{kind:?} edge/{name}");
            }
        }
    }

    #[test]
    fn control_sizes_step_up() {
        for pair in ControlSize::ALL.windows(2) {
            assert_eq!(pair[1].height() - pair[0].height(), space::SM, "{pair:?}");
        }
        assert_eq!(ControlSize::default(), ControlSize::Medium);
    }
}
