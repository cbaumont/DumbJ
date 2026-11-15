use dumbj_settings::{Rgba, Theme, UiRole};
use dumbj_syntax::HighlightKind;
use gpui::{App, Global, Hsla};

pub(crate) struct ActiveTheme(pub(crate) Theme);

impl Global for ActiveTheme {}

fn to_hsla(color: Rgba) -> Hsla {
    gpui::rgba(color.to_u32()).into()
}

pub(crate) fn ui(cx: &App, role: UiRole) -> Hsla {
    to_hsla(cx.global::<ActiveTheme>().0.ui(role))
}

pub(crate) fn syntax(cx: &App, kind: HighlightKind) -> Hsla {
    to_hsla(cx.global::<ActiveTheme>().0.syntax(kind))
}
