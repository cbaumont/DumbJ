use std::ops::Range;
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
use super::file_name;
use super::navigation::{Chooser, ChooserItem, Location};
use crate::theme;

const ROW_HEIGHT: f32 = 22.;
pub(super) const FIND_WINDOW_HEIGHT: f32 = 240.;

pub(super) enum FindRow {
    File {
        relative: String,
        count: usize,
    },
    Usage {
        location: Location,
        line: usize,
        text: String,
        maybe: bool,
        /// Byte ranges of `text` to highlight.
        ranges: Vec<Range<usize>>,
    },
}

pub(super) struct FindWindow {
    title: String,
    rows: Vec<FindRow>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// Text matches are exact; usages are the dumb engine's best guess.
    exact: bool,
    pub(super) visible: bool,
    /// Whether Hide All Tool Windows hid it, so the same action restores it.
    pub(super) hidden_by_hide_all: bool,
}

impl FindWindow {
    pub(super) fn new(title: String, rows: Vec<FindRow>, exact: bool) -> Self {
        let selected = rows
            .iter()
            .position(|row| matches!(row, FindRow::Usage { .. }))
            .unwrap_or(0);
        Self {
            title,
            rows,
            selected,
            scroll: UniformListScrollHandle::new(),
            exact,
            visible: true,
            hidden_by_hide_all: false,
        }
    }

    /// Moves the selection by `delta` result rows, skipping the file headings.
    pub(super) fn step_selection(&mut self, delta: isize) {
        let is_usage = |index: &usize| matches!(self.rows[*index], FindRow::Usage { .. });
        let Some(mut row) = (self.selected..self.rows.len())
            .find(is_usage)
            .or_else(|| (0..self.rows.len()).find(is_usage))
        else {
            return;
        };
        for _ in 0..delta.unsigned_abs() {
            let next = if delta > 0 {
                (row + 1..self.rows.len()).find(is_usage)
            } else {
                (0..row).rev().find(is_usage)
            };
            let Some(next) = next else { break };
            row = next;
        }
        self.selected = row;
        self.scroll
            .scroll_to_item(self.selected.saturating_sub(1), ScrollStrategy::Nearest);
    }

    pub(super) fn selected_location(&self) -> Option<Location> {
        match self.rows.get(self.selected)? {
            FindRow::Usage { location, .. } => Some(location.clone()),
            FindRow::File { .. } => None,
        }
    }
}

fn location(usage: &Usage) -> Location {
    Location {
        path: usage.path.clone(),
        offset: usage.range.start,
    }
}

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
            ranges: Vec::new(),
        }));
    }
    rows
}

fn dumb_summary(name: &str, usages: &[Usage]) -> String {
    let files = usages.chunk_by(|a, b| a.path == b.path).count();
    summary(name, usages.len(), files)
}

pub(super) fn summary(name: &str, usages: usize, files: usize) -> String {
    let plural = |count: usize, word: &str| {
        if count == 1 {
            format!("1 {word}")
        } else {
            format!("{count} {word}s")
        }
    };
    format!(
        "Usages of {name}: {} in {}",
        plural(usages, "usage"),
        plural(files, "file")
    )
}

impl Workspace {
    /// Searches for the usages of the symbol at the caret in the background, then shows
    /// them in a popup (`in_popup`) or in the Find tool window.
    pub(super) fn find_usages(&mut self, in_popup: bool, cx: &mut Context<Self>) {
        let (Some(index), Some(editor)) = (self.active_index(cx), self.active_editor()) else {
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
        let asked = self.ask_exact_usages(cx);
        // With a server asked, the dumb list waits a moment so a quick exact one is not
        // preceded by a flash of guesses.
        let hold = asked
            .is_some()
            .then(|| cx.background_executor().timer(super::smart_nav::DUMB_AFTER));
        let dumb_shown = Arc::new(AtomicBool::new(false));
        if let Some(asked) = asked {
            let (current, dumb_shown, name) = (cancel.clone(), dumb_shown.clone(), name.clone());
            cx.spawn(async move |this, cx| {
                let Some(usages) = asked.await else { return };
                let _ = this.update(cx, |this, cx| {
                    let closed =
                        in_popup && dumb_shown.load(Ordering::Relaxed) && this.chooser.is_none();
                    if closed || !Arc::ptr_eq(&this.usages_cancel, &current) {
                        return;
                    }
                    current.store(true, Ordering::Relaxed);
                    this.show_exact_usages(&name, usages, in_popup, cx);
                });
            })
            .detach();
        }
        cx.spawn(async move |this, cx| {
            let (search_index, flag) = (index.clone(), cancel.clone());
            let usages = cx
                .background_spawn(async move { find_usages(&search_index, &target, &flag) })
                .await;
            if let Some(hold) = hold {
                hold.await;
            }
            let _ = this.update(cx, |this, cx| {
                let Some(usages) = usages.filter(|_| !cancel.load(Ordering::Relaxed)) else {
                    return;
                };
                dumb_shown.store(true, Ordering::Relaxed);
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
                            let label =
                                format!("{}:{}{maybe}", file_name(&usage.path), usage.line + 1);
                            ChooserItem::new(label, usage.line_text.clone(), location(usage))
                        })
                        .collect();
                    this.chooser = Some(Chooser::new(dumb_summary(&name, &usages), items));
                } else {
                    this.find_window = Some(FindWindow::new(
                        dumb_summary(&name, &usages),
                        find_rows(&index, &usages),
                        false,
                    ));
                }
                this.pending_focus = Some(super::PendingFocus::Results);
                cx.notify();
            });
        })
        .detach();
    }

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

    /// `text` with the byte `ranges` on the search-match background.
    fn highlighted(text: &str, ranges: &[Range<usize>], cx: &Context<Self>) -> gpui::Div {
        let mut parts: Vec<(&str, bool)> = Vec::new();
        let mut at = 0;
        for range in ranges {
            let (start, end) = (range.start.max(at), range.end.min(text.len()));
            if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                continue;
            }
            if start > at {
                parts.push((&text[at..start], false));
            }
            parts.push((&text[start..end], true));
            at = end;
        }
        if at < text.len() {
            parts.push((&text[at..], false));
        }
        div()
            .flex()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .children(parts.into_iter().map(|(part, matched)| {
                div()
                    .flex_none()
                    .when(matched, |run| {
                        run.bg(theme::ui(cx, UiRole::EditorSearchMatch))
                    })
                    .child(part.to_owned())
            }))
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
                line,
                text,
                maybe,
                ranges,
                ..
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
                .child(Self::highlighted(text, ranges, cx))
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
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::ui(
                        cx,
                        if find.exact {
                            UiRole::Success
                        } else {
                            UiRole::Warning
                        },
                    ))
                    .child(if find.exact { "exact" } else { "best guess" }),
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
                    ..
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
            dumb_summary("add", &usages),
            "Usages of add: 3 usages in 2 files"
        );
        assert_eq!(
            dumb_summary("x", &usages[..1]),
            "Usages of x: 1 usage in 1 file"
        );
    }
}
