use dumbj_settings::UiRole;
use gpui::{
    App, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Styled,
    Window, div, px,
};

use crate::theme;

pub(super) fn popup_box(cx: &App) -> Div {
    div()
        .rounded(px(8.))
        .bg(theme::ui(cx, UiRole::PopupBackground))
        .border_1()
        .border_color(theme::ui(cx, UiRole::PopupBorder))
        .shadow_lg()
        .text_color(theme::ui(cx, UiRole::Text))
}

pub(super) fn popup_layer<V: 'static>(
    id: &'static str,
    top: f32,
    on_close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    body: impl IntoElement,
    cx: &mut Context<V>,
) -> impl IntoElement {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .pt(px(top))
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| on_close(this, window, cx)),
        )
        .child(
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(body),
        )
}

#[expect(
    clippy::cast_precision_loss,
    reason = "popup lists show a few dozen rows at most"
)]
pub(super) fn list_height(row: f32, count: usize, max: usize) -> Pixels {
    px(row * count.min(max) as f32)
}
