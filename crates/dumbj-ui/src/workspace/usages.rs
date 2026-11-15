//! Find Usages (results in the Find tool window) and Show Usages (results in a popup).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dumbj_actions::Action;
use dumbj_index::{Certainty, Index, Usage, find_usages};
use dumbj_settings::UiRole;
use gpui::{
    Context, IntoElement, MouseButton, ScrollStrategy, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};

use super::Workspace;
use super::navigation::{Chooser, ChooserItem, Location};
use crate::theme;

const ROW_HEIGHT: f32 = 22.;
pub(super) const FIND_WINDOW_HEIGHT: f32 = 240.;

/// A row of the Find tool window.
enum FindRow {
    /// A file heading with the number of usages in it.
    File { relative: String, count: usize },
    Usage {
        location: Location,
        line: usize,
        text: String,
        maybe: bool,
    },
}

/// The Find tool window: the result of the last Find Usages.
pub(super) struct FindWindow {
    title: String,
    rows: Vec<FindRow>,
    selected: usize,
    scroll: UniformListScrollHandle,
    pub(super) visible: bool,
    /// Whether Hide All Tool Windows hid it, so the same action restores it.
    pub(super) hidden_by_hide_all: bool,
}

fn file_name(usage: &Usage) -> String {
    usage
        .path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn location(usage: &Usage) -> Location {
    Location {
        path: usage.path.clone(),
        offset: usage.range.start,
    }
}

/// Groups usages (already sorted by file and position) under file headings.
fn find_rows(index: &Index, usages: &[Usage]) -> Vec<FindRow> {
    let mut rows = Vec::new();
    for group in usages.chunk_by(|a, b| a.path == b.path) {
        let Some(first) = group.first() else { continue };
        let relative = index
            .file_by_path(&first.path)
            .and_then(|id| index.file(id))
            .map_or_else(
                || first.path.display().to_string(),
                |file| file.relative.clone(),
            );
        rows.push(FindRow::File {
            relative,
            count: group.len(),
        });
        rows.extend(group.iter().map(|usage| FindRow::Usage {
            location: location(usage),
            line: usage.line + 1,
            text: usage.line_text.clone(),
            maybe: usage.certainty == Certainty::Maybe,
        }));
    }
    rows
}

fn summary(name: &str, usages: &[Usage]) -> String {
    let files = usages.chunk_by(|a, b| a.path == b.path).count();
    let plural = |count: usize, word: &str| {
        if count == 1 {
            format!("1 {word}")
        } else {
            format!("{count} {word}s")
        }
    };
    format!(
        "Usages of {name}: {} in {}",
        plural(usages.len(), "usage"),
        plural(files, "file")
    )
}

impl Workspace {
    /// Searches for the usages of the symbol at the caret in the background, then shows
    /// them in a popup (`in_popup`) or in the Find tool window.
    pub(super) fn find_usages(&mut self, in_popup: bool, cx: &mut Context<Self>) {
        let (Some(index), Some(editor)) = (self.index.clone(), self.active_editor()) else {
            return;
        };
        let editor = editor.read(cx);
        let (Some(target), Some(name)) =
            (editor.usage_target_at_caret(&index), editor.word_at_caret())
        else {
            self.status_message = Some("No symbol at the caret".to_owned());
            cx.notify();
            return;
        };
        self.usages_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.usages_cancel = cancel.clone();
        self.status_message = Some(format!("Searching for usages of {name}…"));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (search_index, flag) = (index.clone(), cancel.clone());
            let usages = cx
                .background_spawn(async move { find_usages(&search_index, &target, &flag) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(usages) = usages else { return };
                this.status_message = None;
                if usages.is_empty() {
                    this.status_message = Some(format!("No usages of {name} found"));
                } else if in_popup {
                    let items = usages
                        .iter()
                        .map(|usage| {
                            let maybe = if usage.certainty == Certainty::Maybe {
                                " ?"
                            } else {
                                ""
                            };
                            let label = format!("{}:{}{maybe}", file_name(usage), usage.line + 1);
                            ChooserItem::new(label, usage.line_text.clone(), location(usage))
                        })
                        .collect();
                    this.chooser = Some(Chooser::new(summary(&name, &usages), items));
                } else {
                    this.find_window = Some(FindWindow {
                        title: summary(&name, &usages),
                        rows: find_rows(&index, &usages),
                        // The first row is a file heading; start on the first usage.
                        selected: 1,
                        scroll: UniformListScrollHandle::new(),
                        visible: true,
                        hidden_by_hide_all: false,
                    });
                }
                this.focus_results = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows and focuses the Find tool window, or hides it when it already has focus.
    pub(super) fn toggle_find_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.find_window_focus.is_focused(window);
        let Some(find) = self.find_window.as_mut() else {
            return;
        };
        if find.visible && focused {
            find.visible = false;
            self.focus_fallback(window, cx);
        } else {
            find.visible = true;
            let focus = self.find_window_focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn open_find_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find_window.as_mut() else {
            return;
        };
        find.selected = row;
        if let Some(FindRow::Usage { location, .. }) = find.rows.get(row) {
            let location = location.clone();
            self.navigate_to(location, window, cx);
        }
        cx.notify();
    }

    /// Handles a text action while the Find tool window has focus.
    pub(super) fn find_window_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(find) = self.find_window.as_mut() else {
            return;
        };
        let last = find.rows.len().saturating_sub(1);
        let select = |find: &mut FindWindow, row: usize| {
            find.selected = row.min(last);
            find.scroll
                .scroll_to_item(find.selected, ScrollStrategy::Nearest);
        };
        match action {
            Action::MoveUp => select(find, find.selected.saturating_sub(1)),
            Action::MoveDown => select(find, find.selected + 1),
            Action::MovePageUp => select(find, find.selected.saturating_sub(8)),
            Action::MovePageDown => select(find, find.selected + 8),
            Action::MoveTextStart => select(find, 0),
            Action::MoveTextEnd => select(find, last),
            Action::Enter => {
                let selected = find.selected;
                self.open_find_row(selected, window, cx);
            }
            Action::Escape => self.focus_fallback(window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn render_find_row(
        &self,
        find: &FindWindow,
        index: usize,
        row: &FindRow,
        focused: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = index == find.selected;
        let background = match (selected, focused) {
            (true, true) => Some(UiRole::Selected),
            (true, false) => Some(UiRole::SelectedInactive),
            (false, _) => None,
        };
        let content = match row {
            FindRow::File { relative, count } => div()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .text_color(theme::ui(cx, UiRole::Text))
                        .child(relative.clone()),
                )
                .child(
                    div()
                        .text_color(theme::ui(cx, UiRole::TextFaint))
                        .child(count.to_string()),
                ),
            FindRow::Usage {
                line, text, maybe, ..
            } => div()
                .flex()
                .gap(px(10.))
                .pl(px(18.))
                .child(
                    div()
                        .w(px(40.))
                        .flex_none()
                        .text_color(theme::ui(cx, UiRole::TextFaint))
                        .child(line.to_string()),
                )
                .child(div().truncate().child(text.clone()))
                // A usage the dumb engine could not pin to this symbol alone.
                .children(maybe.then(|| {
                    div()
                        .flex_none()
                        .text_color(theme::ui(cx, UiRole::Warning))
                        .child("maybe")
                })),
        };
        let _ = self;
        div()
            .id(("find-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .px(px(12.))
            .when_some(background, |row, role| row.bg(theme::ui(cx, role)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let focus = this.find_window_focus.clone();
                    window.focus(&focus, cx);
                    if event.click_count >= 2 {
                        this.open_find_row(index, window, cx);
                    } else if let Some(find) = this.find_window.as_mut() {
                        find.selected = index;
                        cx.notify();
                    }
                }),
            )
            .child(content)
    }

    pub(super) fn render_find_window(
        &self,
        find: &FindWindow,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focused = self.find_window_focus.is_focused(window);
        let rows = uniform_list(
            "find-rows",
            find.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(find) = this.find_window.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| Some((index, find.rows.get(index)?)))
                    .map(|(index, row)| {
                        this.render_find_row(find, index, row, focused, cx)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&find.scroll)
        .size_full();
        let title = div()
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child(format!("Find: {}", find.title))
            // Dumb-mode usages are heuristic (BRIEF §4.3.1).
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::ui(cx, UiRole::Warning))
                    .child("best guess"),
            );
        div()
            .track_focus(&self.find_window_focus)
            .h(px(FIND_WINDOW_HEIGHT))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_t_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(title)
            .child(div().flex_1().min_h_0().child(rows))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dumbj_text::ByteOffset;

    use super::*;

    fn usage(path: &str, line: usize, certainty: Certainty) -> Usage {
        Usage {
            path: PathBuf::from(path),
            range: ByteOffset(line * 10)..ByteOffset(line * 10 + 3),
            line,
            line_text: format!("line {line}"),
            certainty,
        }
    }

    #[test]
    fn groups_usages_by_file_and_summarises() {
        let usages = [
            usage("/p/A.java", 1, Certainty::Sure),
            usage("/p/A.java", 4, Certainty::Maybe),
            usage("/p/B.kt", 2, Certainty::Sure),
        ];
        let rows: Vec<String> = find_rows(&Index::default(), &usages)
            .iter()
            .map(|row| match row {
                FindRow::File { relative, count } => format!("{relative} ({count})"),
                FindRow::Usage {
                    line,
                    text,
                    maybe,
                    location,
                } => {
                    format!("  {line} {text} {maybe} @{}", location.offset.get())
                }
            })
            .collect();
        assert_eq!(
            rows,
            [
                "/p/A.java (2)",
                "  2 line 1 false @10",
                "  5 line 4 true @40",
                "/p/B.kt (1)",
                "  3 line 2 false @20"
            ]
        );
        assert_eq!(
            summary("add", &usages),
            "Usages of add: 3 usages in 2 files"
        );
        assert_eq!(summary("x", &usages[..1]), "Usages of x: 1 usage in 1 file");
    }
}
