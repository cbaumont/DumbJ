use std::path::PathBuf;

use dumbj_actions::Action;
use dumbj_settings::{NoteSide, ReviewNote, UiRole};
use gpui::{ClipboardItem, Context, Entity, IntoElement, MouseButton, Window, div, prelude::*, px};

use super::Workspace;
use crate::editor::EditorView;
use crate::theme;

/// The lines a note is being written on.
pub(super) struct NotePrompt {
    path: PathBuf,
    side: NoteSide,
    first_line: usize,
    last_line: usize,
    excerpt: String,
}

impl Workspace {
    /// The review file, side and editor the caret is in, when the active tab is one of the
    /// review's diffs.
    pub(super) fn note_site(&self) -> Option<(PathBuf, NoteSide, Entity<EditorView>)> {
        let index = self.active_review_file()?;
        let path = self.review_file_path(index)?;
        let diff = self.active_diff()?;
        let side = if diff.left_active() {
            NoteSide::Base
        } else {
            NoteSide::Head
        };
        Some((path, side, self.active_editor()?))
    }

    pub(super) fn can_add_note(&self) -> bool {
        self.state.review.is_some() && self.note_site().is_some()
    }

    pub(super) fn has_review_notes(&self) -> bool {
        let saved = self.state.review.as_ref();
        self.review.is_some() && saved.is_some_and(|review| !review.notes().is_empty())
    }

    /// The note on the caret's line, for the status bar.
    pub(super) fn note_at_caret(&self, cx: &gpui::App) -> Option<String> {
        let (path, side, editor) = self.note_site()?;
        let line = editor.read(cx).caret_line_column().0;
        let note = self.state.review.as_ref()?.note_at(&path, side, line)?;
        let text: String = note.text.chars().take(80).collect();
        Some(format!("Note: {text}"))
    }

    pub(super) fn open_note_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, side, editor)) = self.note_site() else {
            return;
        };
        let editor = editor.read(cx);
        let (mut first_line, mut last_line) = editor.selected_lines();
        let caret = editor.caret_line_column().0;
        let existing = self
            .state
            .review
            .as_ref()
            .and_then(|review| review.note_at(&path, side, caret));
        let text = existing.map_or_else(String::new, |note| note.text.clone());
        if let Some(note) = existing {
            (first_line, last_line) = (note.first_line, note.last_line);
        }
        let excerpt = editor.lines_text(first_line, last_line);
        self.note_prompt = Some(NotePrompt {
            path,
            side,
            first_line,
            last_line,
            excerpt,
        });
        self.note_input
            .update(cx, |input, cx| input.set_text(&text, true, cx));
        let handle = self.note_input.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn close_note_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.note_prompt = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    pub(super) fn note_prompt_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Enter => {
                let text = self.note_input.read(cx).text().to_owned();
                if let (Some(prompt), Some(review)) =
                    (self.note_prompt.take(), self.state.review.as_mut())
                {
                    review.set_note(ReviewNote {
                        path: prompt.path,
                        side: prompt.side,
                        first_line: prompt.first_line,
                        last_line: prompt.last_line,
                        text,
                        excerpt: prompt.excerpt,
                    });
                    self.save_project_state(cx);
                    self.refresh_note_marks(cx);
                }
                self.close_note_prompt(window, cx);
            }
            Action::Escape => self.close_note_prompt(window, cx),
            _ => {
                self.note_input
                    .update(cx, |input, cx| input.handle_action(action, window, cx));
            }
        }
    }

    /// Shows the noted lines in the gutters of the review's open diffs.
    pub(super) fn refresh_note_marks(&self, cx: &mut Context<Self>) {
        let (Some(review), Some(repository)) = (&self.state.review, &self.repository) else {
            return;
        };
        for tab in &self.tabs {
            let Some(diff) = tab.diff.as_ref().filter(|diff| self.diff_in_review(diff)) else {
                continue;
            };
            let Ok(path) = tab.path.strip_prefix(repository.workdir()) else {
                continue;
            };
            for (editor, side) in [(diff.left(), NoteSide::Base), (&tab.editor, NoteSide::Head)] {
                let lines: Vec<(usize, usize)> = review
                    .notes_for(path)
                    .filter(|note| note.side == side)
                    .map(|note| (note.first_line, note.last_line))
                    .collect();
                editor.update(cx, |editor, cx| editor.set_note_lines(lines, cx));
            }
        }
    }

    pub(super) fn export_review_notes(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &self.state.review else {
            return;
        };
        let count = review.notes().len();
        cx.write_to_clipboard(ClipboardItem::new_string(review.to_markdown()));
        let noun = if count == 1 { "note" } else { "notes" };
        self.status_message = Some(format!("Copied {count} {noun} as Markdown"));
        cx.notify();
    }

    pub(super) fn render_note_prompt(
        &self,
        prompt: &NotePrompt,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lines = if prompt.last_line > prompt.first_line {
            format!("lines {}–{}", prompt.first_line, prompt.last_line)
        } else {
            format!("line {}", prompt.first_line)
        };
        let side = match prompt.side {
            NoteSide::Base => " (base)",
            NoteSide::Head => "",
        };
        let popup = div()
            .w(px(640.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(8.))
            .bg(theme::ui(cx, UiRole::PopupBackground))
            .border_1()
            .border_color(theme::ui(cx, UiRole::PopupBorder))
            .shadow_lg()
            .text_color(theme::ui(cx, UiRole::Text))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(format!("Note on {}, {lines}{side}", prompt.path.display()))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::ui(cx, UiRole::TextFaint))
                            .child("Enter saves · empty removes"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .child(self.note_input.clone()),
            );
        div()
            .id("note-prompt-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(140.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_note_prompt(window, cx)),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(popup),
            )
    }
}
