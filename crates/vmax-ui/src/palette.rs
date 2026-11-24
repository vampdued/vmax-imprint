//! Configurable theme palette adapter for multi-app reuse.

use egui::{Color32, CornerRadius, Stroke, Visuals};

use crate::tokens::{
    ACCENT, ACCENT_HOVER, ACCENT_PRESSED, BG_CONTROL, BG_CONTROL_ACTIVE, BG_CONTROL_HOVER,
    BG_ELEVATED, BG_PANEL, BORDER_STRONG, BORDER_SUBTLE, FOCUS_RING, RADIUS_MD, TEXT_PRIMARY,
    TEXT_SECONDARY,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VmaxPalette {
    pub panel_fill: Color32,
    pub elevated_fill: Color32,
    pub control_idle: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub border_subtle: Color32,
    pub border_strong: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub focus_ring: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
}

impl Default for VmaxPalette {
    fn default() -> Self {
        Self::vmax_graphite()
    }
}

impl VmaxPalette {
    pub const fn vmax_graphite() -> Self {
        Self {
            panel_fill: BG_PANEL,
            elevated_fill: BG_ELEVATED,
            control_idle: BG_CONTROL,
            control_hover: BG_CONTROL_HOVER,
            control_active: BG_CONTROL_ACTIVE,
            border_subtle: BORDER_SUBTLE,
            border_strong: BORDER_STRONG,
            accent: ACCENT,
            accent_hover: ACCENT_HOVER,
            accent_pressed: ACCENT_PRESSED,
            focus_ring: FOCUS_RING,
            text_primary: TEXT_PRIMARY,
            text_secondary: TEXT_SECONDARY,
        }
    }

    pub const fn with_accent(
        mut self,
        accent: Color32,
        accent_hover: Color32,
        accent_pressed: Color32,
        focus_ring: Color32,
    ) -> Self {
        self.accent = accent;
        self.accent_hover = accent_hover;
        self.accent_pressed = accent_pressed;
        self.focus_ring = focus_ring;
        self
    }

    pub fn apply_to(self, visuals: &mut Visuals) {
        let radius = CornerRadius::same(RADIUS_MD);
        visuals.panel_fill = self.panel_fill;
        visuals.window_fill = self.elevated_fill;
        visuals.extreme_bg_color = self.panel_fill;
        visuals.selection.bg_fill = self.accent;
        visuals.selection.stroke = Stroke::new(1.5_f32, self.focus_ring);

        visuals.widgets.noninteractive.bg_fill = self.panel_fill;
        visuals.widgets.noninteractive.weak_bg_fill = self.panel_fill;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.border_subtle);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text_secondary);
        visuals.widgets.noninteractive.corner_radius = radius;

        visuals.widgets.inactive.bg_fill = self.control_idle;
        visuals.widgets.inactive.weak_bg_fill = self.control_idle;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, self.border_subtle);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.inactive.corner_radius = radius;

        visuals.widgets.hovered.bg_fill = self.control_hover;
        visuals.widgets.hovered.weak_bg_fill = self.control_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.border_strong);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.hovered.corner_radius = radius;

        visuals.widgets.active.bg_fill = self.control_active;
        visuals.widgets.active.weak_bg_fill = self.control_active;
        visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, self.focus_ring);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.active.corner_radius = radius;

        visuals.widgets.open.bg_fill = self.control_hover;
        visuals.widgets.open.weak_bg_fill = self.control_hover;
        visuals.widgets.open.bg_stroke = Stroke::new(1.5_f32, self.focus_ring);
        visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        visuals.widgets.open.corner_radius = radius;
    }
}
