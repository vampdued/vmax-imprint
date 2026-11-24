#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo,
        clippy::unreachable
    )
)]

//! `vmax-ui` (egui 0.37 edition) — Shared VMAX Brand Kit v1.2 tokens, WCAG helpers,
//! [`VmaxPalette`], [`ToolbarFit`], and panic-free custom `egui` primitives.

pub mod layout;
pub mod palette;
pub mod tokens;
pub mod widgets;

pub use layout::{ToolbarFit, fit_toolbar_groups};
pub use palette::VmaxPalette;
pub use tokens::*;
pub use widgets::*;
