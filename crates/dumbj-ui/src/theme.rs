//! The active theme as a GPUI global, with colours converted to GPUI's types.

use dumbj_settings::{Rgba, Theme, UiRole};
use dumbj_syntax::HighlightKind;
use gpui::{App, Global, Hsla};

/// The theme every view paints with.
pub(crate) struct ActiveTheme(pub(crate) Theme);

impl Global for ActiveTheme {}

fn to_hsla(color: Rgba) -> Hsla {
    gpui::rgba(color.to_u32()).into()
}

/// The colour of a UI role in the active theme.
pub(crate) fn ui(cx: &App, role: UiRole) -> Hsla {
    to_hsla(cx.global::<ActiveTheme>().0.ui(role))
}

/// The colour of a syntax role in the active theme.
pub(crate) fn syntax(cx: &App, kind: HighlightKind) -> Hsla {
    to_hsla(cx.global::<ActiveTheme>().0.syntax(kind))
}
