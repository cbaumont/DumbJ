use std::path::PathBuf;
use std::time::Duration;

use dumbj_actions::Action;
use dumbj_lsp::{GotoKind, from_lsp, goto, hover, uri_to_path};
use dumbj_settings::UiRole;
use dumbj_syntax::ReferenceRole;
use dumbj_text::{Buffer, ByteOffset};
use gpui::{
    AppContext, Context, IntoElement, MouseButton, ScrollStrategy, UniformListScrollHandle, Window,
    div, prelude::*, px, uniform_list,
};

use super::Workspace;
use super::smart_nav::DUMB_AFTER;
use crate::assets::EDITOR_FONT;
use crate::theme;

const ROW_HEIGHT: f32 = 20.;
const VISIBLE_ROWS: usize = 22;
const MAX_DEFINITION_LINES: usize = 60;
const GIVE_UP_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PeekKind {
    Documentation,
    Definition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeekLine {
    text: String,
    code: bool,
}

pub(super) struct Peek {
    generation: u64,
    title: String,
    exact: bool,
    lines: Vec<PeekLine>,
    top: usize,
    scroll: UniformListScrollHandle,
}

fn code(lines: Vec<String>) -> Vec<PeekLine> {
    lines
        .into_iter()
        .map(|text| PeekLine { text, code: true })
        .collect()
}

/// The declaration that starts on `line`, to the end of its block or statement.
fn definition_lines(buffer: &Buffer, line: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut depth = 0_i32;
    let mut opened = false;
    for index in line..buffer.line_count().min(line + MAX_DEFINITION_LINES) {
        let text = buffer.line(dumbj_text::LineIndex(index));
        let text = text.trim_end();
        for character in text.chars() {
            match character {
                '{' | '(' => {
                    depth += 1;
                    opened |= character == '{';
                }
                '}' | ')' => depth -= 1,
                _ => {}
            }
        }
        lines.push(text.to_owned());
        let statement_ended = !opened && (text.ends_with(';') || text.is_empty());
        if depth <= 0 && (opened || statement_ended || index > line) {
            break;
        }
    }
    let indent = lines
        .iter()
        .filter(|text| !text.trim().is_empty())
        .map(|text| text.len() - text.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .into_iter()
        .map(|text| text.get(indent..).unwrap_or("").to_owned())
        .collect()
}

/// The comment written right above the declaration on `line`, without its markers.
fn doc_lines(buffer: &Buffer, line: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for index in (0..line).rev() {
        let text = buffer.line(dumbj_text::LineIndex(index));
        let text = text.trim();
        if text.starts_with('@') && lines.is_empty() {
            continue;
        }
        let comment = text.starts_with("/*")
            || text.starts_with('*')
            || text.starts_with("//")
            || text.ends_with("*/");
        if !comment {
            break;
        }
        let cleaned = text
            .trim_start_matches("/**")
            .trim_start_matches("/*")
            .trim_start_matches("///")
            .trim_start_matches("//")
            .trim_end_matches("*/")
            .trim_start_matches('*')
            .trim();
        lines.push(cleaned.to_owned());
        if text.starts_with("/*") {
            break;
        }
    }
    lines.reverse();
    let first = lines.iter().position(|text| !text.is_empty());
    let last = lines.iter().rposition(|text| !text.is_empty());
    match (first, last) {
        (Some(first), Some(last)) => lines.drain(first..=last).collect(),
        _ => Vec::new(),
    }
}

/// A line of Markdown prose as plain text: list markers become bullets, links their
/// text, and emphasis and code marks go.
fn plain(line: &str) -> String {
    let line = line.trim_start_matches('#').trim_start();
    let (bullet, line) = match line.strip_prefix("* ").or_else(|| line.strip_prefix("- ")) {
        Some(rest) => ("• ", rest),
        None => ("", line),
    };
    let mut text = String::from(bullet);
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some((label, tail)) = after
            .split_once("](")
            .and_then(|(label, tail)| Some((label, tail.split_once(')')?.1)))
        else {
            break;
        };
        text.push_str(&rest[..open]);
        text.push_str(label);
        rest = tail;
    }
    text.push_str(rest);
    text.replace(['*', '`'], "").replace("__", "")
}

fn markdown_lines(markdown: &str) -> Vec<PeekLine> {
    let mut code = false;
    let mut lines = Vec::new();
    for text in markdown.lines() {
        if text.trim_start().starts_with("```") {
            code = !code;
            continue;
        }
        let text = text.trim_end();
        lines.push(PeekLine {
            text: if code { text.to_owned() } else { plain(text) },
            code,
        });
    }
    lines
}

fn peek_lines(kind: PeekKind, path: &PathBuf, at: impl Fn(&Buffer) -> ByteOffset) -> Vec<PeekLine> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let buffer = Buffer::new(&text);
    let line = buffer.point_at(at(&buffer)).line.get();
    match kind {
        PeekKind::Definition => code(definition_lines(&buffer, line)),
        PeekKind::Documentation => {
            let mut lines: Vec<PeekLine> = doc_lines(&buffer, line)
                .into_iter()
                .map(|text| PeekLine { text, code: false })
                .collect();
            if !lines.is_empty() {
                lines.push(PeekLine {
                    text: String::new(),
                    code: false,
                });
                lines.push(PeekLine {
                    text: buffer.line(dumbj_text::LineIndex(line)).trim().to_owned(),
                    code: true,
                });
            }
            lines
        }
    }
}

impl Workspace {
    pub(super) fn peek_open(&self) -> bool {
        self.smart.peek.is_some()
    }

    /// Where the dumb engine thinks the symbol at the caret is declared.
    fn dumb_peek_target(&self, cx: &Context<Self>) -> Option<(PathBuf, ByteOffset)> {
        let index = self.active_index(cx)?;
        let editor = self.active_editor()?.read(cx);
        let resolution = editor.resolve_at_caret(&index)?;
        if resolution.reference.role == ReferenceRole::Declaration {
            return Some((editor.path().to_path_buf(), editor.caret_offset()));
        }
        let target = resolution.targets.first()?;
        Some((target.path.clone(), target.range.start))
    }

    pub(super) fn quick_peek(&mut self, kind: PeekKind, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let Some(name) = editor.read(cx).word_at_caret() else {
            self.status_message = Some("No symbol at the caret".to_owned());
            cx.notify();
            return;
        };
        let title = match kind {
            PeekKind::Documentation => format!("Documentation of {name}"),
            PeekKind::Definition => format!("Definition of {name}"),
        };
        self.smart.generation += 1;
        let generation = self.smart.generation;
        self.smart.peek = None;

        let asked = self.current_location(cx).and_then(|origin| {
            let server = self.smart_server(&origin.path)?;
            let position = editor.read(cx).lsp_caret()?;
            Some(cx.background_spawn(async move {
                match kind {
                    PeekKind::Documentation => {
                        hover(&server, &origin.path, position, GIVE_UP_AFTER)
                            .ok()
                            .flatten()
                            .map(|markdown| markdown_lines(&markdown))
                    }
                    PeekKind::Definition => {
                        let places = goto(
                            &server,
                            GotoKind::Definition,
                            &origin.path,
                            position,
                            GIVE_UP_AFTER,
                        )
                        .ok()?;
                        let place = places.first()?;
                        let path = uri_to_path(&place.uri)?;
                        let start = place.range.start;
                        Some(peek_lines(kind, &path, |buffer| from_lsp(buffer, start)))
                    }
                }
                .filter(|lines| !lines.is_empty())
            }))
        });
        let hold = asked
            .is_some()
            .then(|| cx.background_executor().timer(DUMB_AFTER));
        if let Some(asked) = asked {
            let title = title.clone();
            cx.spawn(async move |this, cx| {
                let Some(lines) = asked.await else { return };
                let _ = this.update(cx, |this, cx| {
                    this.show_peek(generation, title, lines, true, cx);
                });
            })
            .detach();
        }
        let target = self.dumb_peek_target(cx);
        cx.spawn(async move |this, cx| {
            let lines = cx
                .background_spawn(async move {
                    target.map_or_else(Vec::new, |(path, offset)| {
                        peek_lines(kind, &path, |_| offset)
                    })
                })
                .await;
            if let Some(hold) = hold {
                hold.await;
            }
            let _ = this.update(cx, |this, cx| {
                if this.smart.peek.as_ref().is_some_and(|peek| peek.exact) {
                    return;
                }
                if lines.is_empty() {
                    if this.smart.generation == generation {
                        this.status_message = Some(match kind {
                            PeekKind::Documentation => "No documentation found".to_owned(),
                            PeekKind::Definition => "No definition found".to_owned(),
                        });
                        cx.notify();
                    }
                } else {
                    this.show_peek(generation, title, lines, false, cx);
                }
            });
        })
        .detach();
    }

    fn show_peek(
        &mut self,
        generation: u64,
        title: String,
        lines: Vec<PeekLine>,
        exact: bool,
        cx: &mut Context<Self>,
    ) {
        // An exact answer replaces the guess only while that guess is still on screen.
        let replaces = self
            .smart
            .peek
            .as_ref()
            .is_none_or(|peek| peek.generation == generation);
        let closed = exact && self.smart.peek.is_none() && self.smart.peek_closed == generation;
        if self.smart.generation != generation || !replaces || closed {
            return;
        }
        self.status_message = None;
        self.smart.peek = Some(Peek {
            generation,
            title,
            exact,
            lines,
            top: 0,
            scroll: UniformListScrollHandle::new(),
        });
        self.pending_focus = Some(super::PendingFocus::Results);
        cx.notify();
    }

    pub(super) fn peek_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(peek) = self.smart.peek.as_mut() else {
            return;
        };
        let last = peek.lines.len().saturating_sub(VISIBLE_ROWS);
        match action {
            Action::MoveUp => peek.top = peek.top.saturating_sub(1),
            Action::MoveDown => peek.top = (peek.top + 1).min(last),
            Action::MovePageUp => peek.top = peek.top.saturating_sub(VISIBLE_ROWS),
            Action::MovePageDown => peek.top = (peek.top + VISIBLE_ROWS).min(last),
            Action::MoveTextStart => peek.top = 0,
            Action::MoveTextEnd => peek.top = last,
            Action::Escape | Action::Enter => {
                self.smart.peek_closed = peek.generation;
                self.smart.peek = None;
                self.focus_fallback(window, cx);
            }
            _ => {}
        }
        if let Some(peek) = self.smart.peek.as_ref() {
            peek.scroll.scroll_to_item(peek.top, ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub(super) fn render_peek(peek: &Peek, cx: &mut Context<Self>) -> impl IntoElement {
        let count = peek.lines.len();
        #[expect(clippy::cast_precision_loss, reason = "a popup shows a few dozen rows")]
        let height = px(ROW_HEIGHT * count.min(VISIBLE_ROWS) as f32);
        let rows = uniform_list(
            "peek-rows",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                let Some(peek) = this.smart.peek.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| peek.lines.get(index))
                    .map(|line| {
                        div()
                            .h(px(ROW_HEIGHT))
                            .px(px(10.))
                            .whitespace_nowrap()
                            .when(line.code, |row| {
                                row.font_family(EDITOR_FONT)
                                    .text_color(theme::ui(cx, UiRole::Text))
                            })
                            .child(line.text.clone())
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&peek.scroll)
        .h(height);
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(10.))
            .pb(px(6.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child(peek.title.clone())
            .child(
                div()
                    .px(px(6.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .text_color(theme::ui(cx, UiRole::Warning))
                    .text_size(px(11.))
                    .child(if peek.exact { "exact" } else { "best guess" }),
            );
        let popup = div()
            .w(px(820.))
            .flex()
            .flex_col()
            .p(px(8.))
            .rounded(px(8.))
            .bg(theme::ui(cx, UiRole::PopupBackground))
            .border_1()
            .border_color(theme::ui(cx, UiRole::PopupBorder))
            .shadow_lg()
            .text_size(px(13.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .overflow_hidden()
            .child(header)
            .child(rows);
        div()
            .id("peek-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(140.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.peek_action(Action::Escape, window, cx)),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(popup),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "class A {\n    /**\n     * Says hello.\n     *\n     * @return the greeting\n     */\n    @Override\n    public String greet(\n            String name) {\n        if (name == null) {\n            return \"\";\n        }\n        return name;\n    }\n\n    int count = 0;\n    int other;\n}\n";

    #[test]
    fn a_definition_runs_to_the_end_of_its_block_and_loses_its_indent() {
        let lines = definition_lines(&Buffer::new(SOURCE), 7);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("public String greet(")
        );
        assert_eq!(lines.last().map(String::as_str), Some("}"));
        assert_eq!(lines.len(), 7);
    }

    #[test]
    fn a_field_is_one_line() {
        assert_eq!(
            definition_lines(&Buffer::new(SOURCE), 15),
            ["int count = 0;"]
        );
    }

    #[test]
    fn the_comment_above_is_the_documentation_past_annotations() {
        let lines = doc_lines(&Buffer::new(SOURCE), 7);
        assert_eq!(lines, ["Says hello.", "", "@return the greeting"]);
        assert_eq!(doc_lines(&Buffer::new(SOURCE), 15), Vec::<String>::new());
    }

    #[test]
    fn prose_loses_its_markdown() {
        assert_eq!(plain("* Returns:"), "• Returns:");
        assert_eq!(
            plain("Source: *[demo](file:///p/Greeter.java#3)* and [x](y)"),
            "Source: demo and x"
        );
        assert_eq!(plain("a [b] c"), "a [b] c");
    }

    #[test]
    fn markdown_fences_mark_code_lines() {
        let lines = markdown_lines("```java\nvoid f()\n```\n## **Returns:** `x`");
        let seen: Vec<_> = lines
            .iter()
            .map(|line| (line.text.as_str(), line.code))
            .collect();
        assert_eq!(seen, [("void f()", true), ("Returns: x", false)]);
    }
}
