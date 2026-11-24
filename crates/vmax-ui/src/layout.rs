//! Responsive toolbar overflow geometry (`ToolbarFit`).

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolbarFit {
    pub shown: Vec<(usize, f32)>,
    pub overflow: Vec<usize>,
    pub overflow_width: f32,
}

impl ToolbarFit {
    pub fn all_visible(&self) -> bool {
        self.overflow.is_empty()
    }
}

pub fn fit_toolbar_groups(
    group_widths: &[f32],
    available_width: f32,
    spacing: f32,
    overflow_trigger_w: f32,
) -> ToolbarFit {
    if group_widths.is_empty() {
        return ToolbarFit::default();
    }
    let avail = available_width.max(0.0);
    let gap = spacing.max(0.0);
    let trig_w = overflow_trigger_w.max(0.0);

    let mut total = 0.0_f32;
    for (idx, &w) in group_widths.iter().enumerate() {
        if idx > 0 {
            total += gap;
        }
        total += w.max(0.0);
    }
    if total <= avail {
        return ToolbarFit {
            shown: group_widths
                .iter()
                .enumerate()
                .map(|(i, &w)| (i, w.max(0.0)))
                .collect(),
            overflow: Vec::new(),
            overflow_width: 0.0,
        };
    }

    let mut shown = Vec::new();
    let mut overflow = Vec::new();
    let mut used = 0.0_f32;
    let mut spilling = false;

    for (idx, &raw_w) in group_widths.iter().enumerate() {
        let w = raw_w.max(0.0);
        if spilling {
            overflow.push(idx);
            continue;
        }
        let next_used = if shown.is_empty() { w } else { used + gap + w };
        if next_used + gap + trig_w <= avail {
            shown.push((idx, w));
            used = next_used;
        } else {
            spilling = true;
            overflow.push(idx);
        }
    }

    ToolbarFit {
        shown,
        overflow,
        overflow_width: trig_w,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_width_spills_trailing_groups() {
        let fit = fit_toolbar_groups(&[100.0, 80.0, 60.0], 220.0, 8.0, 28.0);
        assert_eq!(fit.shown, vec![(0, 100.0)]);
        assert_eq!(fit.overflow, vec![1, 2]);
        assert_eq!(fit.overflow_width, 28.0);
    }
}
