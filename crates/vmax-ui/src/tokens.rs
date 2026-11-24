//! VMAX Brand Kit v1.2 dark surface hierarchy, semantic colors, radii, heights, and WCAG 2.1 math.

use egui::Color32;

// 4-Tier Dark Surface Hierarchy & Borders
pub const BG_CANVAS: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x17, 0x18, 0x1B);
pub const BG_ELEVATED: Color32 = Color32::from_rgb(0x1E, 0x20, 0x26);
pub const BG_CONTROL: Color32 = Color32::from_rgb(0x25, 0x28, 0x30);
pub const BG_CONTROL_HOVER: Color32 = Color32::from_rgb(0x2F, 0x33, 0x3E);
pub const BG_CONTROL_ACTIVE: Color32 = Color32::from_rgb(0x39, 0x3E, 0x4B);
pub const BG_RECESSED: Color32 = Color32::from_rgb(0x0E, 0x10, 0x14);
pub const BG_ERROR_BANNER: Color32 = Color32::from_rgb(0x6E, 0x1A, 0x1A);

pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x2A, 0x2E, 0x38);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x62, 0x6B, 0x80);
pub const FOCUS_RING: Color32 = Color32::from_rgb(0x8F, 0xA8, 0xFF);

// Typography Colors (WCAG AA/AAA verified)
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xC3, 0xC9, 0xD1);
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x8C, 0x94, 0xA3);

// Brand & Semantic Colors
pub const ACCENT: Color32 = Color32::from_rgb(0x2F, 0x5B, 0xFF); // Max Blue
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x44, 0x6D, 0xFF);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x1E, 0x46, 0xDB);
pub const CUT_DANGER: Color32 = Color32::from_rgb(0xFF, 0x45, 0x3A);
pub const CUT_DANGER_FILL: Color32 = Color32::from_rgb(0xC8, 0x1E, 0x1E);

pub const STATUS_TEAL_BG: Color32 = Color32::from_rgb(0x16, 0x24, 0x28);
pub const STATUS_TEAL_BORDER: Color32 = Color32::from_rgb(0x2A, 0x4A, 0x50);
pub const STATUS_TEAL_TEXT: Color32 = Color32::from_rgb(0x7D, 0xD3, 0xC0);

pub const INFO_BANNER_BG: Color32 = Color32::from_rgb(0x18, 0x20, 0x33);
pub const INFO_BANNER_BORDER: Color32 = Color32::from_rgb(0x33, 0x47, 0x73);
pub const INFO_BANNER_TEXT: Color32 = Color32::from_rgb(0xD8, 0xE2, 0xFF);

// Radii & Control Heights
pub const RADIUS_SM: u8 = 4;
pub const RADIUS_MD: u8 = 6;
pub const RADIUS_LG: u8 = 10;

pub const BTN_H_SM: f32 = 24.0;
pub const BTN_H_MD: f32 = 26.0;
pub const BTN_H_LG: f32 = 28.0;

pub fn accent_soft() -> Color32 {
    Color32::from_rgba_unmultiplied(0x2F, 0x5B, 0xFF, 0x2E)
}

pub fn relative_luminance(c: Color32) -> f64 {
    let chan = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * chan(c.r()) + 0.7152 * chan(c.g()) + 0.0722 * chan(c.b())
}

pub fn contrast_ratio(fg: Color32, bg: Color32) -> f64 {
    let l1 = relative_luminance(fg);
    let l2 = relative_luminance(bg);
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_contrast_thresholds_hold() {
        assert!(contrast_ratio(TEXT_PRIMARY, BG_PANEL) >= 15.0);
        assert!(contrast_ratio(TEXT_SECONDARY, BG_PANEL) >= 10.0);
        assert!(contrast_ratio(TEXT_PRIMARY, ACCENT) >= 4.5);
        assert!(contrast_ratio(TEXT_PRIMARY, CUT_DANGER_FILL) >= 4.5);
        assert!(contrast_ratio(FOCUS_RING, BG_PANEL) >= 7.0);
        assert!(contrast_ratio(BORDER_STRONG, BG_PANEL) >= 3.0);
    }
}
