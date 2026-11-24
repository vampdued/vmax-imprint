//! Custom VMAX UI primitives for `egui 0.37` (exact-height inputs, sliders, progress bar,
//! toggle switch, badges, section frames, and buttons).

use std::ops::RangeInclusive;

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, TextStyle, Ui, Vec2,
};

use crate::tokens::{
    ACCENT, ACCENT_HOVER, ACCENT_PRESSED, BG_CONTROL, BG_CONTROL_ACTIVE, BG_CONTROL_HOVER,
    BG_ELEVATED, BG_ERROR_BANNER, BG_RECESSED, BORDER_STRONG, BORDER_SUBTLE, BTN_H_LG, BTN_H_MD,
    BTN_H_SM, CUT_DANGER, CUT_DANGER_FILL, FOCUS_RING, INFO_BANNER_BG, INFO_BANNER_BORDER,
    INFO_BANNER_TEXT, RADIUS_MD, RADIUS_SM, STATUS_TEAL_BG, STATUS_TEAL_BORDER, STATUS_TEAL_TEXT,
    TEXT_DISABLED, TEXT_PRIMARY, TEXT_SECONDARY,
};

/// Semantic tone for [`status_badge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeTone {
    Neutral,
    Info,
    Success,
    Danger,
}

/// Primary Max Blue CTA button (`28 px` default height).
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    styled_button(ui, label, BTN_H_LG, true, false, true)
}

/// Tonal secondary button at an explicit flush height (`BTN_H_SM`, `BTN_H_MD`, or `BTN_H_LG`).
pub fn tonal_button(ui: &mut Ui, label: &str, height: f32) -> Response {
    styled_button(ui, label, height, false, false, true)
}

/// Tonal secondary button with explicit enabled state.
pub fn tonal_button_enabled(ui: &mut Ui, label: &str, height: f32, enabled: bool) -> Response {
    styled_button(ui, label, height, false, false, enabled)
}

/// Destructive danger button (`#C81E1E` high-contrast fill).
pub fn danger_tonal_button(ui: &mut Ui, label: &str, height: f32) -> Response {
    styled_button(ui, label, height, false, true, true)
}

fn styled_button(
    ui: &mut Ui,
    label: &str,
    height: f32,
    primary: bool,
    danger: bool,
    enabled: bool,
) -> Response {
    let fg = if !enabled {
        TEXT_DISABLED
    } else {
        TEXT_PRIMARY
    };
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::proportional(12.5),
        fg,
    );
    let w = (galley.size().x + 22.0).max(52.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), sense);
    let fill = if !enabled {
        BG_ELEVATED
    } else if primary {
        if resp.is_pointer_button_down_on() {
            ACCENT_PRESSED
        } else if resp.hovered() {
            ACCENT_HOVER
        } else {
            ACCENT
        }
    } else if danger {
        CUT_DANGER_FILL
    } else if resp.is_pointer_button_down_on() {
        BG_CONTROL_ACTIVE
    } else if resp.hovered() {
        BG_CONTROL_HOVER
    } else {
        BG_CONTROL
    };
    let stroke = if !enabled {
        Stroke::new(1.0_f32, BORDER_SUBTLE)
    } else if resp.has_focus() {
        Stroke::new(1.5_f32, FOCUS_RING)
    } else if primary {
        Stroke::new(1.0_f32, ACCENT)
    } else if danger {
        Stroke::new(1.0_f32, CUT_DANGER)
    } else if resp.hovered() {
        Stroke::new(1.0_f32, BORDER_STRONG)
    } else {
        Stroke::new(1.0_f32, BORDER_SUBTLE)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS_MD), fill);
    ui.painter()
        .rect_stroke(rect, CornerRadius::same(RADIUS_MD), stroke, egui::StrokeKind::Inside);
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.5), fg);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Custom VMAX horizontal `f64` slider with 3 px recessed track, `ACCENT` fill, 10 px thumb,
/// optional quantization `step`, keyboard arrows, and focus ring.
pub fn slider_f64(
    ui: &mut Ui,
    value: &mut f64,
    range: RangeInclusive<f64>,
    step: Option<f64>,
    width: f32,
    height: f32,
    enabled: bool,
) -> Response {
    let min_v = *range.start();
    let max_v = *range.end();
    let span = (max_v - min_v).max(f64::EPSILON);
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(width.max(48.0), height), sense);
    let track_pad = 6.0_f32;
    let track_left = rect.left() + track_pad;
    let track_right = (rect.right() - track_pad).max(track_left + 1.0);
    let track_w = track_right - track_left;

    let quantize = |v: f64| -> f64 {
        let clamped = v.clamp(min_v, max_v);
        if let Some(s) = step {
            if s > 0.0 {
                return (((clamped - min_v) / s).round() * s + min_v).clamp(min_v, max_v);
            }
        }
        clamped
    };

    if enabled && (resp.clicked() || resp.dragged()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = f64::from(((pos.x - track_left) / track_w).clamp(0.0, 1.0));
            let new_v = quantize(min_v + t * span);
            if (new_v - *value).abs() > 1e-6 {
                *value = new_v;
                resp.mark_changed();
            }
        }
    }
    if enabled && resp.has_focus() {
        let k_step = step.unwrap_or(span * 0.05);
        if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            let new_v = quantize(*value - k_step);
            if (new_v - *value).abs() > 1e-6 {
                *value = new_v;
                resp.mark_changed();
            }
        } else if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            let new_v = quantize(*value + k_step);
            if (new_v - *value).abs() > 1e-6 {
                *value = new_v;
                resp.mark_changed();
            }
        }
    }

    let frac = ((*value - min_v) / span).clamp(0.0, 1.0) as f32;
    let track_rect = Rect::from_min_max(
        Pos2::new(track_left, rect.center().y - 2.0),
        Pos2::new(track_right, rect.center().y + 2.0),
    );
    ui.painter()
        .rect_filled(track_rect, CornerRadius::same(2), BG_RECESSED);
    ui.painter().rect_stroke(
        track_rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, BORDER_SUBTLE),
        egui::StrokeKind::Inside,
    );

    let fill_color = if enabled { ACCENT } else { TEXT_DISABLED };
    let thumb_x = track_left + track_w * frac;
    if frac > 0.0 {
        let fill_rect = Rect::from_min_max(
            track_rect.min,
            Pos2::new(thumb_x.max(track_left + 2.0), track_rect.max.y),
        );
        ui.painter()
            .rect_filled(fill_rect, CornerRadius::same(2), fill_color);
    }

    let thumb_r = if enabled && (resp.hovered() || resp.dragged()) {
        6.0_f32
    } else {
        5.0_f32
    };
    ui.painter().circle_filled(
        Pos2::new(thumb_x, rect.center().y),
        thumb_r,
        if enabled { TEXT_PRIMARY } else { TEXT_DISABLED },
    );
    ui.painter().circle_stroke(
        Pos2::new(thumb_x, rect.center().y),
        thumb_r,
        Stroke::new(1.5_f32, fill_color),
    );
    if enabled && resp.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(RADIUS_SM),
            Stroke::new(1.5_f32, FOCUS_RING),
            egui::StrokeKind::Inside,
        );
    }
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Integer `i32` slider backed by [`slider_f64`].
pub fn slider_i32(
    ui: &mut Ui,
    value: &mut i32,
    range: RangeInclusive<i32>,
    width: f32,
    height: f32,
    enabled: bool,
) -> Response {
    let mut f = f64::from(*value);
    let min_f = f64::from(*range.start());
    let max_f = f64::from(*range.end());
    let mut resp = slider_f64(ui, &mut f, min_f..=max_f, Some(1.0), width, height, enabled);
    let next = f.round() as i32;
    if next != *value {
        *value = next;
        resp.mark_changed();
    }
    resp
}

/// Custom VMAX `8 px` progress bar with optional animated pulse.
pub fn progress_bar(ui: &mut Ui, fraction: f32, width: f32, running: bool) {
    let frac = fraction.clamp(0.0, 1.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(60.0), 8.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), BG_RECESSED);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0_f32, BORDER_SUBTLE),
        egui::StrokeKind::Inside,
    );
    if frac > 0.0 {
        let fill_w = (rect.width() * frac).max(8.0).min(rect.width());
        let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
        ui.painter()
            .rect_filled(fill_rect, CornerRadius::same(4), ACCENT);
        if running && frac < 1.0 {
            let t = (ui.input(|i| i.time) as f32 * 0.9) % 1.0;
            let pulse_w = (fill_w * 0.35).clamp(16.0, 56.0);
            let px = fill_rect.left() + (fill_w + pulse_w) * t - pulse_w;
            let pulse_rect =
                Rect::from_min_size(Pos2::new(px, rect.top()), Vec2::new(pulse_w, rect.height()));
            ui.painter().with_clip_rect(fill_rect).rect_filled(
                pulse_rect,
                CornerRadius::same(4),
                Color32::from_rgba_unmultiplied(143, 168, 255, 85),
            );
            ui.ctx().request_repaint();
        }
    }
}

/// Progress bar paired with a right-aligned tabular monospace percentage readout.
pub fn progress_bar_with_label(ui: &mut Ui, fraction: f32, width: f32, running: bool) {
    let pct = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        progress_bar(ui, fraction, (width - 42.0).max(80.0), running);
        ui.label(
            egui::RichText::new(format!("{pct:>3}%"))
                .monospace()
                .size(12.0)
                .color(TEXT_SECONDARY),
        );
    });
}

/// Compact pill status badge (`Neutral`, `Info`, `Success`, `Danger`).
pub fn status_badge(ui: &mut Ui, label: &str, tone: BadgeTone) -> Response {
    let (bg, border, fg) = match tone {
        BadgeTone::Neutral => (BG_RECESSED, BORDER_SUBTLE, TEXT_SECONDARY),
        BadgeTone::Info => (INFO_BANNER_BG, INFO_BANNER_BORDER, INFO_BANNER_TEXT),
        BadgeTone::Success => (STATUS_TEAL_BG, STATUS_TEAL_BORDER, STATUS_TEAL_TEXT),
        BadgeTone::Danger => (BG_ERROR_BANNER, CUT_DANGER, TEXT_PRIMARY),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(11.5), fg);
    let w = (galley.size().x + 16.0).max(48.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, BTN_H_SM), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS_SM), bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(RADIUS_SM),
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.5), fg);
    resp
}

/// Custom VMAX pill toggle switch (`34×18 px` track + `12 px` thumb).
pub fn toggle_switch(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> Response {
    let track_w = 34.0_f32;
    let track_h = 18.0_f32;
    let text_w = if label.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(label.to_string(), FontId::proportional(12.5), TEXT_PRIMARY)
            .size()
            .x
            + 8.0
    };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(track_w + text_w, BTN_H_SM), sense);
    let key_toggle = enabled
        && resp.has_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter));
    if (enabled && resp.clicked()) || key_toggle {
        *on = !*on;
        resp.mark_changed();
    }
    let track_rect = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - track_h * 0.5),
        Vec2::new(track_w, track_h),
    );
    let track_fill = if !enabled {
        BG_ELEVATED
    } else if *on {
        if resp.hovered() { ACCENT_HOVER } else { ACCENT }
    } else if resp.hovered() {
        BG_CONTROL_HOVER
    } else {
        BG_RECESSED
    };
    let track_stroke = if !enabled {
        Stroke::new(1.0_f32, BORDER_SUBTLE)
    } else if resp.has_focus() {
        Stroke::new(1.5_f32, FOCUS_RING)
    } else if *on {
        Stroke::new(1.0_f32, ACCENT)
    } else {
        Stroke::new(1.0_f32, BORDER_STRONG)
    };
    ui.painter()
        .rect_filled(track_rect, CornerRadius::same(9), track_fill);
    ui.painter()
        .rect_stroke(track_rect, CornerRadius::same(9), track_stroke, egui::StrokeKind::Inside);
    let knob_x = if *on {
        track_rect.right() - 9.0
    } else {
        track_rect.left() + 9.0
    };
    ui.painter().circle_filled(
        Pos2::new(knob_x, track_rect.center().y),
        6.0_f32,
        if enabled { TEXT_PRIMARY } else { TEXT_DISABLED },
    );
    if !label.is_empty() {
        ui.painter().text(
            Pos2::new(track_rect.right() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            if enabled { TEXT_PRIMARY } else { TEXT_DISABLED },
        );
    }
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Options for [`text_input`].
pub struct TextInputOpts<'a> {
    pub width: f32,
    pub height: f32,
    pub hint: &'a str,
    pub monospace: bool,
    pub invalid: bool,
    pub enabled: bool,
}

impl<'a> TextInputOpts<'a> {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            height: BTN_H_MD,
            hint: "",
            monospace: false,
            invalid: false,
            enabled: true,
        }
    }
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }
    pub fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Exact-height recessed single-line text input (zero vertical padding overflow).
pub fn text_input(ui: &mut Ui, buf: &mut String, opts: TextInputOpts<'_>) -> Response {
    let radius = if opts.height <= BTN_H_SM {
        RADIUS_SM
    } else {
        RADIUS_MD
    };
    let fill = if opts.enabled { BG_RECESSED } else { BG_ELEVATED };
    let pad_x = 7.0_f32;
    let inner_w = (opts.width - pad_x * 2.0).max(32.0);
    let (outer_rect, box_resp) =
        ui.allocate_exact_size(Vec2::new(opts.width, opts.height), Sense::hover());
    ui.painter()
        .rect_filled(outer_rect, CornerRadius::same(radius), fill);

    let text_h = 16.0_f32.min((opts.height - 4.0).max(12.0));
    let content_rect = Rect::from_center_size(
        outer_rect.center(),
        Vec2::new(inner_w, text_h),
    );
    let mut child_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child_ui.spacing_mut().interact_size.y = text_h;
    #[allow(deprecated)]
    let mut te = egui::TextEdit::singleline(buf)
        .frame(egui::Frame::NONE)
        .desired_width(inner_w)
        .vertical_align(egui::Align::Center)
        .interactive(opts.enabled);
    if opts.monospace {
        te = te.font(TextStyle::Monospace);
    }
    if !opts.hint.is_empty() {
        te = te.hint_text(egui::RichText::new(opts.hint).color(TEXT_DISABLED));
    }
    if !opts.enabled {
        te = te.text_color(TEXT_DISABLED);
    }
    let te_resp = child_ui.add_sized(Vec2::new(inner_w, text_h), te);

    let border = if opts.invalid {
        Stroke::new(1.5_f32, CUT_DANGER)
    } else if te_resp.has_focus() {
        Stroke::new(1.5_f32, FOCUS_RING)
    } else if !opts.enabled {
        Stroke::new(1.0_f32, BORDER_SUBTLE)
    } else if box_resp.hovered() || te_resp.hovered() {
        Stroke::new(1.0_f32, FOCUS_RING)
    } else {
        Stroke::new(1.0_f32, BORDER_STRONG)
    };
    ui.painter()
        .rect_stroke(outer_rect, CornerRadius::same(radius), border, egui::StrokeKind::Inside);
    te_resp
}

/// Elevated section container with optional title.
pub fn section_frame(ui: &mut Ui, title: Option<&str>, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(BG_ELEVATED)
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if let Some(heading) = title {
                if !heading.is_empty() {
                    ui.label(
                        egui::RichText::new(heading)
                            .size(13.0)
                            .strong()
                            .color(TEXT_PRIMARY),
                    );
                    ui.add_space(6.0);
                }
            }
            add_contents(ui);
        });
}

/// 1 px horizontal divider.
pub fn divider_h(ui: &mut Ui) {
    let w = ui.available_width().max(8.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 5.0), Sense::hover());
    ui.painter().line_segment(
        [
            Pos2::new(rect.left(), rect.center().y),
            Pos2::new(rect.right(), rect.center().y),
        ],
        Stroke::new(1.0_f32, BORDER_SUBTLE),
    );
}

/// 1 px vertical divider matched to a toolbar row height.
pub fn divider_v(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(5.0, height), Sense::hover());
    ui.painter().line_segment(
        [
            Pos2::new(rect.center().x, rect.top() + 3.0),
            Pos2::new(rect.center().x, rect.bottom() - 3.0),
        ],
        Stroke::new(1.0_f32, BORDER_SUBTLE),
    );
}
