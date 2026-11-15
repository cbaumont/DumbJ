//! The element and the `EntityInputHandler` wiring are adapted from Zed's GPUI example
//! `crates/gpui/examples/input.rs` (Apache-2.0) at revision
//! e8bc16acee5ae1937600d571d441c4b5d7b0d288.

use std::ops::Range;

use dumbj_actions::Action;
use dumbj_settings::UiRole;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, Pixels, Point, Render, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, div, fill, point,
    prelude::*, px, relative, size,
};

use crate::line_edit::{LineEdit, LineMotion};
use crate::theme;

pub(crate) struct TextChanged;

pub(crate) struct TextInput {
    focus: FocusHandle,
    edit: LineEdit,
    placeholder: SharedString,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl EventEmitter<TextChanged> for TextInput {}

impl TextInput {
    pub(crate) fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            edit: LineEdit::default(),
            placeholder: placeholder.into(),
            last_layout: None,
            last_bounds: None,
        }
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub(crate) fn text(&self) -> &str {
        self.edit.text()
    }

    pub(crate) fn set_text(&mut self, text: &str, select_all: bool, cx: &mut Context<Self>) {
        self.edit.set_text(text, select_all);
        cx.notify();
    }

    pub(crate) fn select_all(&mut self, cx: &mut Context<Self>) {
        self.edit.select_all();
        cx.notify();
    }

    fn changed(window: &mut Window, cx: &mut Context<Self>) {
        window.invalidate_character_coordinates();
        cx.emit(TextChanged);
        cx.notify();
    }

    /// Runs a text action. Returns `false` if the field has no use for it, so the caller
    /// can handle it (Enter and Escape, for instance).
    pub(crate) fn handle_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let motion = |motion, extend| Some((motion, extend));
        let movement = match action {
            Action::MoveLeft => motion(LineMotion::Left, false),
            Action::MoveRight => motion(LineMotion::Right, false),
            Action::MoveWordLeft => motion(LineMotion::WordLeft, false),
            Action::MoveWordRight => motion(LineMotion::WordRight, false),
            Action::MoveLineStart | Action::MoveTextStart => motion(LineMotion::Start, false),
            Action::MoveLineEnd | Action::MoveTextEnd => motion(LineMotion::End, false),
            Action::SelectLeft => motion(LineMotion::Left, true),
            Action::SelectRight => motion(LineMotion::Right, true),
            Action::SelectWordLeft => motion(LineMotion::WordLeft, true),
            Action::SelectWordRight => motion(LineMotion::WordRight, true),
            Action::SelectLineStart | Action::SelectTextStart => motion(LineMotion::Start, true),
            Action::SelectLineEnd | Action::SelectTextEnd => motion(LineMotion::End, true),
            _ => None,
        };
        if let Some((motion, extend)) = movement {
            self.edit.apply_motion(motion, extend);
            cx.notify();
            return true;
        }
        match action {
            Action::SelectAll => self.select_all(cx),
            Action::Backspace | Action::Delete => {
                if self.edit.delete(action == Action::Delete) {
                    Self::changed(window, cx);
                }
            }
            Action::Copy | Action::Cut => {
                let selected = self.edit.selected_text().to_owned();
                if !selected.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(selected));
                    if action == Action::Cut {
                        self.edit.replace(None, "");
                        Self::changed(window, cx);
                    }
                }
            }
            Action::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.edit.replace(None, &text);
                    Self::changed(window, cx);
                }
            }
            _ => return false,
        }
        true
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if let (Some(bounds), Some(layout)) = (self.last_bounds, self.last_layout.as_ref()) {
            let index = layout.closest_index_for_x(event.position.x - bounds.left());
            self.edit.move_to(index, event.modifiers.shift);
            cx.notify();
        }
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.edit.utf16_to_range(&range_utf16);
        adjusted_range.replace(self.edit.range_to_utf16(&range));
        self.edit.text().get(range).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.edit.range_to_utf16(&self.edit.selection()),
            reversed: self.edit.is_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.edit
            .marked()
            .map(|range| self.edit.range_to_utf16(&range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.edit.unmark();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|range| self.edit.utf16_to_range(&range));
        self.edit.replace(range, text);
        Self::changed(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|range| self.edit.utf16_to_range(&range));
        // The new selection is in UTF-16 units relative to the start of `new_text`.
        let selected = new_selected_range_utf16.map(|selected| {
            let to_utf8 = |units: usize| {
                let prefix: Vec<u16> = new_text.encode_utf16().take(units).collect();
                String::from_utf16_lossy(&prefix).len().min(new_text.len())
            };
            to_utf8(selected.start)..to_utf8(selected.end)
        });
        self.edit.replace_and_mark(range, new_text, selected);
        Self::changed(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.edit.utf16_to_range(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self.last_bounds?.localize(&point)?;
        let index = self.last_layout.as_ref()?.closest_index_for_x(local.x);
        Some(self.edit.offset_to_utf16(index))
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .flex_1()
            .min_w_0()
            .cursor(CursorStyle::IBeam)
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .child(TextInputElement { input: cx.entity() })
    }
}

struct TextInputElement {
    input: Entity<TextInput>,
}

struct InputLayout {
    line: ShapedLine,
    selection: Option<gpui::PaintQuad>,
    caret: Option<gpui::PaintQuad>,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = InputLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let (text, color) = if input.edit.text().is_empty() {
            (input.placeholder.clone(), theme::ui(cx, UiRole::TextFaint))
        } else {
            (
                SharedString::from(input.edit.text().to_owned()),
                style.color,
            )
        };
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match input
            .edit
            .marked()
            .filter(|_| !input.edit.text().is_empty())
        {
            Some(marked) => {
                let underline = UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.),
                    wavy: false,
                };
                vec![
                    TextRun {
                        len: marked.start,
                        ..run.clone()
                    },
                    TextRun {
                        len: marked.len(),
                        underline: Some(underline),
                        ..run.clone()
                    },
                    TextRun {
                        len: text.len() - marked.end,
                        ..run
                    },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect()
            }
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text, font_size, &runs, None);

        let selection = input.edit.selection();
        let x = |index: usize| bounds.left() + line.x_for_index(index);
        let selection_quad = (!selection.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(x(selection.start), bounds.top()),
                    point(x(selection.end), bounds.bottom()),
                ),
                theme::ui(cx, UiRole::EditorSelection),
            )
        });
        let caret_quad = fill(
            Bounds::new(
                point(x(input.edit.caret()), bounds.top()),
                size(px(1.5), bounds.size.height),
            ),
            theme::ui(cx, UiRole::EditorCaret),
        );
        InputLayout {
            line,
            selection: selection_quad,
            caret: Some(caret_quad),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = layout.selection.take() {
            window.paint_quad(selection);
        }
        if let Err(error) = layout.line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        ) {
            tracing::warn!(%error, "failed to paint text input");
        }
        if focus.is_focused(window)
            && let Some(caret) = layout.caret.take()
        {
            window.paint_quad(caret);
        }
        let line = layout.line.clone();
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}
