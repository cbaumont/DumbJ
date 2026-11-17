use std::path::PathBuf;

use dumbj_actions::Action;
use dumbj_settings::{Bookmark, UiRole};
use gpui::{Context, IntoElement, Keystroke, Window, div, prelude::*, px};

use super::Workspace;
use super::file_name;
use super::navigation::{Chooser, ChooserItem};
use super::popup::{popup_box, popup_layer};
use super::state::is_project_file;
use crate::editor::BookmarkMark;
use crate::theme;

pub(super) struct MnemonicPrompt {
    path: PathBuf,
    line: usize,
    text: String,
}

fn is_mnemonic(key: char) -> bool {
    key.is_ascii_alphanumeric()
}

fn typed_mnemonic(keystroke: &Keystroke) -> Option<char> {
    let modifiers = &keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }
    let mut chars = keystroke.key.chars();
    let key = chars.next().filter(|key| is_mnemonic(*key))?;
    chars.next().is_none().then_some(key.to_ascii_uppercase())
}

fn chooser_item(bookmark: &Bookmark) -> ChooserItem {
    let marker = bookmark.mnemonic.unwrap_or('●');
    ChooserItem::at_line(
        format!("{marker}  {}:{}", file_name(&bookmark.path), bookmark.line),
        bookmark.text.clone(),
        bookmark.path.clone(),
        bookmark.line,
        bookmark.mnemonic,
    )
}

impl Workspace {
    fn bookmark_site(&self, cx: &Context<Self>) -> Option<(PathBuf, usize, String)> {
        let tab = self.active_tab().filter(|_| !self.in_revision())?;
        if self.project.is_none() || !is_project_file(&tab.path) {
            return None;
        }
        let editor = tab.editor.read(cx);
        let text = editor.caret_line_text()?;
        Some((tab.path.clone(), editor.caret_line_column().0, text))
    }

    pub(super) fn bookmarks_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh_bookmark_marks(cx);
        self.save_project_state(cx);
        cx.notify();
    }

    pub(super) fn refresh_bookmark_marks(&self, cx: &mut Context<Self>) {
        for tab in self.tabs.iter().filter(|tab| tab.shows_working_file()) {
            let marks = self
                .state
                .bookmarks
                .iter()
                .filter(|bookmark| bookmark.path == tab.path)
                .map(|bookmark| BookmarkMark {
                    line: bookmark.line,
                    mnemonic: bookmark.mnemonic,
                })
                .collect();
            tab.editor
                .update(cx, |editor, cx| editor.set_bookmarks(marks, cx));
        }
    }

    pub(super) fn toggle_bookmark(&mut self, cx: &mut Context<Self>) {
        let Some((path, line, text)) = self.bookmark_site(cx) else {
            return;
        };
        let added = self.state.toggle_bookmark(&path, line, &text);
        self.status_message = Some(format!(
            "Bookmark {} on line {line}",
            if added { "added" } else { "removed" }
        ));
        self.bookmarks_changed(cx);
    }

    pub(super) fn toggle_bookmark_mnemonic(&mut self, mnemonic: char, cx: &mut Context<Self>) {
        let Some((path, line, text)) = self.bookmark_site(cx) else {
            return;
        };
        let added = self
            .state
            .toggle_bookmark_with_mnemonic(&path, line, mnemonic, &text);
        let mnemonic = mnemonic.to_ascii_uppercase();
        self.status_message = Some(if added {
            format!("Bookmark {mnemonic} set on line {line}")
        } else {
            format!("Bookmark {mnemonic} removed from line {line}")
        });
        self.bookmarks_changed(cx);
    }

    pub(super) fn open_mnemonic_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line, text)) = self.bookmark_site(cx) else {
            return;
        };
        self.mnemonic_prompt = Some(MnemonicPrompt { path, line, text });
        // Like the chooser, the prompt has no text field; keys reach it through the window.
        let focus = self.focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_mnemonic_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mnemonic_prompt = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    pub(super) fn goto_bookmark(
        &mut self,
        mnemonic: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bookmark) = self.state.bookmark_with_mnemonic(mnemonic).cloned() else {
            return;
        };
        self.navigate_to_line(bookmark.path, bookmark.line, window, cx);
    }

    pub(super) fn show_bookmarks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.bookmarks.is_empty() {
            self.status_message = Some("No bookmarks in this project".to_owned());
            cx.notify();
            return;
        }
        let items = self.state.bookmarks.iter().map(chooser_item).collect();
        self.chooser = Some(Chooser::exact("Bookmarks".to_owned(), items));
        let focus = self.focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn navigate_to_line(
        &mut self,
        path: PathBuf,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.record_current_location(cx);
        let from = self.current_location(cx);
        self.open_file(path, window, cx);
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.go_to(line, None, cx));
        }
        if let (Some(from), Some(to)) = (from, self.current_location(cx)) {
            self.history.record_jump(from, to);
        }
    }

    pub(super) fn handle_bookmark_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(prompt) = self.mnemonic_prompt.take() {
            if let Some(mnemonic) = typed_mnemonic(keystroke) {
                let added = self.state.toggle_bookmark_with_mnemonic(
                    &prompt.path,
                    prompt.line,
                    mnemonic,
                    &prompt.text,
                );
                self.status_message = Some(format!(
                    "Bookmark {mnemonic} {} line {}",
                    if added { "set on" } else { "removed from" },
                    prompt.line
                ));
                self.bookmarks_changed(cx);
                self.focus_fallback(window, cx);
            } else if keystroke.key == "escape" {
                self.focus_fallback(window, cx);
                cx.notify();
            } else {
                self.mnemonic_prompt = Some(prompt);
            }
            return true;
        }
        let picked =
            typed_mnemonic(keystroke).and_then(|key| self.chooser.as_ref()?.item_for_key(key));
        if let Some(index) = picked {
            self.accept_chooser(index, window, cx);
            return true;
        }
        false
    }

    pub(super) fn render_mnemonic_prompt(
        &self,
        prompt: &MnemonicPrompt,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let used: String = self
            .state
            .bookmarks
            .iter()
            .filter_map(|bookmark| bookmark.mnemonic)
            .collect();
        let popup = popup_box(cx)
            .w(px(380.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .child(
                div()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(Action::ToggleBookmarkWithMnemonic.title()),
            )
            .child(format!(
                "Press a digit or a letter for {}:{}",
                file_name(&prompt.path),
                prompt.line
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(if used.is_empty() {
                        "No mnemonics in use".to_owned()
                    } else {
                        format!("In use: {used}")
                    }),
            );
        popup_layer(
            "mnemonic-layer",
            140.,
            Workspace::close_mnemonic_prompt,
            popup,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::Modifiers;

    use super::*;

    fn press(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: None,
        }
    }

    #[test]
    fn plain_digits_and_letters_are_mnemonics() {
        let none = Modifiers::default();
        assert_eq!(typed_mnemonic(&press("3", none)), Some('3'));
        assert_eq!(typed_mnemonic(&press("q", none)), Some('Q'));
        let shift = Modifiers::shift();
        assert_eq!(typed_mnemonic(&press("q", shift)), Some('Q'));
    }

    #[test]
    fn named_keys_and_chords_are_not() {
        let none = Modifiers::default();
        assert_eq!(typed_mnemonic(&press("escape", none)), None);
        assert_eq!(typed_mnemonic(&press("enter", none)), None);
        assert_eq!(typed_mnemonic(&press("-", none)), None);
        assert_eq!(typed_mnemonic(&press("q", Modifiers::control())), None);
        assert_eq!(typed_mnemonic(&press("q", Modifiers::alt())), None);
    }

    #[test]
    fn a_bookmark_lists_its_mnemonic_and_place() {
        let bookmark = |mnemonic| Bookmark {
            path: PathBuf::from("/p/Cart.java"),
            line: 12,
            text: "int total;".to_owned(),
            mnemonic,
        };
        let item = chooser_item(&bookmark(Some('A')));
        assert_eq!(item.label, "A  Cart.java:12");
        assert_eq!(item.detail, "int total;");
        assert_eq!(item.line, Some(12));
        assert_eq!(item.key, Some('A'));
        assert_eq!(chooser_item(&bookmark(None)).label, "●  Cart.java:12");
    }
}
