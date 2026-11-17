use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dumbj_actions::Action;
use dumbj_search::{Outcome, SearchError, SearchOptions, find_in_files};
use dumbj_settings::UiRole;
use dumbj_text::ByteOffset;
use gpui::{
    Context, IntoElement, MouseButton, SharedString, Task, Window, div, prelude::*, px, svg,
};

use super::Workspace;
use super::file_name;
use super::navigation::Location;
use super::popup::{popup_box, popup_layer};
use super::usages::{FindRow, FindWindow};
use crate::text_input::TextInput;
use crate::theme;

const MAX_MATCHES: usize = 2000;
const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SearchScope {
    #[default]
    Project,
    Directory,
}

#[derive(Debug, Clone, Copy, Default)]
struct Toggles {
    regex: bool,
    match_case: bool,
    whole_word: bool,
    scope: SearchScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Idle,
    Searching,
    Done(String),
    Capped(String),
    Failed(String),
}

pub(super) struct FindInFiles {
    toggles: Toggles,
    directory: Option<PathBuf>,
    status: Status,
    cancel: Arc<AtomicBool>,
    debounce: Option<Task<()>>,
}

impl Drop for FindInFiles {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn plural(count: usize, word: &str) -> String {
    match count {
        1 => format!("1 {word}"),
        _ if word.ends_with('h') => format!("{count} {word}es"),
        _ => format!("{count} {word}s"),
    }
}

fn find_rows(root: &std::path::Path, outcome: &Outcome) -> Vec<FindRow> {
    let mut rows = Vec::new();
    for group in outcome.matches.chunk_by(|a, b| a.path == b.path) {
        let Some(first) = group.first() else { continue };
        rows.push(FindRow::File {
            relative: first
                .path
                .strip_prefix(root)
                .unwrap_or(&first.path)
                .display()
                .to_string(),
            count: group.len(),
        });
        rows.extend(group.iter().map(|found| FindRow::Usage {
            location: Location {
                path: found.path.clone(),
                offset: ByteOffset(found.offset),
            },
            line: found.line + 1,
            text: found.text.clone(),
            maybe: false,
            ranges: found.ranges.clone(),
        }));
    }
    rows
}

fn summary(outcome: &Outcome) -> String {
    let files = outcome.matches.chunk_by(|a, b| a.path == b.path).count();
    let found = format!(
        "{} in {}",
        plural(outcome.matches.len(), "match"),
        plural(files, "file")
    );
    if outcome.capped {
        format!("{found} (showing the first {MAX_MATCHES}; narrow the search)")
    } else {
        found
    }
}

impl Workspace {
    pub(super) fn open_find_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|project| project.root.clone()) else {
            return;
        };
        let directory = self
            .active_tab()
            .and_then(|tab| tab.path.parent().map(std::path::Path::to_path_buf))
            .filter(|directory| directory.starts_with(&root));
        let seed = self
            .active_editor()
            .and_then(|editor| editor.read(cx).selection_for_search());
        self.search = None;
        self.chooser = None;
        self.goto_line_open = false;
        let reopened = self.find_in_files.is_some();
        let toggles = self
            .find_in_files
            .as_ref()
            .map_or_else(Toggles::default, |open| open.toggles);
        self.find_in_files = Some(FindInFiles {
            toggles,
            directory,
            status: Status::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
            debounce: None,
        });
        self.find_query.update(cx, |input, cx| match seed {
            Some(seed) => input.set_text(&seed, true, cx),
            None => input.select_all(cx),
        });
        let focus = self.find_query.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        if !reopened || !self.find_query.read(cx).text().is_empty() {
            self.schedule_find_in_files(cx);
        }
        cx.notify();
    }

    pub(super) fn close_find_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_in_files = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    pub(super) fn schedule_find_in_files(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.find_in_files.as_mut() else {
            return;
        };
        open.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        open.cancel = cancel.clone();
        open.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.run_find_in_files(&cancel, cx));
        }));
        cx.notify();
    }

    fn run_find_in_files(&mut self, cancel: &Arc<AtomicBool>, cx: &mut Context<Self>) {
        let (Some(project), Some(open)) = (self.project.as_ref(), self.find_in_files.as_mut())
        else {
            return;
        };
        let query = self.find_query.read(cx).text().to_owned();
        if query.is_empty() {
            open.status = Status::Idle;
            cx.notify();
            return;
        }
        let root = project.root.clone();
        let scope = match (&open.directory, open.toggles.scope) {
            (Some(directory), SearchScope::Directory) => directory.clone(),
            _ => root.clone(),
        };
        let options = SearchOptions {
            query,
            regex: open.toggles.regex,
            match_case: open.toggles.match_case,
            whole_word: open.toggles.whole_word,
            file_mask: self.find_mask.read(cx).text().to_owned(),
        };
        open.status = Status::Searching;
        let cancel = cancel.clone();
        let label = if scope == root {
            "project".to_owned()
        } else {
            file_name(&scope)
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (flag, query) = (cancel.clone(), options.query.clone());
            let search_root = root.clone();
            let outcome = cx
                .background_spawn(async move {
                    find_in_files(&search_root, &scope, &options, MAX_MATCHES, &flag)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let Some(open) = this.find_in_files.as_mut() else {
                    return;
                };
                match outcome {
                    Ok(outcome) => {
                        let summary = summary(&outcome);
                        open.status = if outcome.capped {
                            Status::Capped(summary.clone())
                        } else {
                            Status::Done(summary.clone())
                        };
                        this.find_window = Some(FindWindow::new(
                            format!("“{query}” in {label}: {summary}"),
                            find_rows(&root, &outcome),
                            true,
                        ));
                    }
                    Err(SearchError::Cancelled) => {}
                    Err(error) => open.status = Status::Failed(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn find_in_files_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Escape => self.close_find_in_files(window, cx),
            Action::Enter => {
                let target = self
                    .find_window
                    .as_ref()
                    .and_then(FindWindow::selected_location);
                self.close_find_in_files(window, cx);
                if let Some(target) = target {
                    self.navigate_to(target, window, cx);
                }
            }
            Action::MoveUp | Action::MoveDown | Action::MovePageUp | Action::MovePageDown => {
                let delta = match action {
                    Action::MoveUp => -1,
                    Action::MoveDown => 1,
                    Action::MovePageUp => -8,
                    _ => 8,
                };
                if let Some(find) = self.find_window.as_mut() {
                    find.step_selection(delta);
                }
            }
            Action::Tab => {
                let (query, mask) = (
                    self.find_query.read(cx).focus_handle().clone(),
                    self.find_mask.read(cx).focus_handle().clone(),
                );
                window.focus(
                    if mask.is_focused(window) {
                        &query
                    } else {
                        &mask
                    },
                    cx,
                );
            }
            _ => {
                let input = if self.find_mask.read(cx).focus_handle().is_focused(window) {
                    &self.find_mask
                } else {
                    &self.find_query
                };
                input.update(cx, |input, cx| input.handle_action(action, window, cx));
            }
        }
        cx.notify();
    }

    fn toggle_find_option(&mut self, change: impl FnOnce(&mut Toggles), cx: &mut Context<Self>) {
        if let Some(open) = self.find_in_files.as_mut() {
            change(&mut open.toggles);
            self.schedule_find_in_files(cx);
        }
    }

    fn option_chip(
        id: &'static str,
        label: SharedString,
        on: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .px(px(6.))
            .py(px(2.))
            .rounded(px(4.))
            .text_size(px(12.))
            .text_color(theme::ui(
                cx,
                if on { UiRole::Text } else { UiRole::TextMuted },
            ))
            .when(on, |chip| chip.bg(theme::ui(cx, UiRole::Selected)))
            .when(!on, |chip| {
                chip.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(label)
    }

    fn render_find_options(toggles: Toggles, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap(px(4.))
            .child(
                Self::option_chip("fif-case", "Cc".into(), toggles.match_case, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.toggle_find_option(|t| t.match_case = !t.match_case, cx);
                    }),
                ),
            )
            .child(
                Self::option_chip("fif-word", "W".into(), toggles.whole_word, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.toggle_find_option(|t| t.whole_word = !t.whole_word, cx);
                    }),
                ),
            )
            .child(
                Self::option_chip("fif-regex", ".*".into(), toggles.regex, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.toggle_find_option(|t| t.regex = !t.regex, cx);
                    }),
                ),
            )
    }

    fn render_find_scope(
        directory: &std::path::Path,
        toggles: Toggles,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let name = file_name(directory);
        let chip = |id: &'static str, label: String, scope: SearchScope, cx: &mut Context<Self>| {
            Self::option_chip(id, label.into(), toggles.scope == scope, cx).on_click(
                cx.listener(move |this, _, _, cx| this.toggle_find_option(|t| t.scope = scope, cx)),
            )
        };
        div()
            .flex()
            .gap(px(4.))
            .child(chip(
                "fif-project",
                "Project".to_owned(),
                SearchScope::Project,
                cx,
            ))
            .child(chip("fif-directory", name, SearchScope::Directory, cx))
    }

    fn render_find_field(
        input: &gpui::Entity<TextInput>,
        icon: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let focused = input.read(cx).focus_handle().is_focused(window);
        div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(4.))
            .bg(theme::ui(cx, UiRole::InputBackground))
            .border_1()
            .border_color(if focused {
                theme::ui(cx, UiRole::Accent)
            } else {
                gpui::transparent_black()
            })
            .when(icon, |field| {
                field.child(
                    svg()
                        .path("icons/search.svg")
                        .size(px(14.))
                        .flex_none()
                        .text_color(theme::ui(cx, UiRole::TextFaint)),
                )
            })
            .child(input.clone())
    }

    pub(super) fn render_find_in_files(
        &self,
        open: &FindInFiles,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (status, role) = match &open.status {
            Status::Idle => (String::new(), UiRole::TextFaint),
            Status::Searching => ("Searching…".to_owned(), UiRole::TextMuted),
            Status::Done(text) => (text.clone(), UiRole::TextMuted),
            Status::Capped(text) => (text.clone(), UiRole::Warning),
            Status::Failed(text) => (text.clone(), UiRole::Error),
        };
        let scope = open
            .directory
            .as_ref()
            .map(|directory| Self::render_find_scope(directory, open.toggles, cx));
        let panel = popup_box(cx)
            .w(px(680.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(theme::ui(cx, UiRole::TextMuted))
                            .child(Action::FindInPath.title().trim_end_matches('…').to_owned()),
                    )
                    .child(Self::render_find_options(open.toggles, cx)),
            )
            .child(
                div()
                    .flex()
                    .child(Self::render_find_field(&self.find_query, true, window, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_color(theme::ui(cx, UiRole::TextMuted))
                            .child("File mask"),
                    )
                    .child(Self::render_find_field(&self.find_mask, false, window, cx))
                    .children(scope),
            )
            .child(
                div()
                    .h(px(16.))
                    .text_size(px(12.))
                    .text_color(theme::ui(cx, role))
                    .child(status),
            );
        popup_layer(
            "find-in-files-layer",
            110.,
            Workspace::close_find_in_files,
            panel,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dumbj_search::Match;

    use super::*;

    fn found(path: &str, line: usize) -> Match {
        Match {
            path: PathBuf::from(path),
            line,
            text: format!("text {line}"),
            ranges: std::iter::once(0..4).collect(),
            offset: line * 10,
        }
    }

    #[test]
    fn rows_group_matches_by_file_with_project_relative_names() {
        let outcome = Outcome {
            matches: vec![
                found("/p/a/A.java", 1),
                found("/p/a/A.java", 4),
                found("/p/B.kt", 0),
            ],
            capped: false,
        };
        let rows: Vec<String> = find_rows(Path::new("/p"), &outcome)
            .iter()
            .map(|row| match row {
                FindRow::File { relative, count } => format!("{relative} ({count})"),
                FindRow::Usage {
                    line,
                    location,
                    ranges,
                    ..
                } => format!("  {line} @{} {ranges:?}", location.offset.get()),
            })
            .collect();
        assert_eq!(
            rows,
            [
                "a/A.java (2)",
                "  2 @10 [0..4]",
                "  5 @40 [0..4]",
                "B.kt (1)",
                "  1 @0 [0..4]"
            ]
        );
    }

    #[test]
    fn the_summary_counts_and_says_when_capped() {
        let mut outcome = Outcome {
            matches: vec![found("/p/A", 0), found("/p/A", 1), found("/p/B", 0)],
            ..Outcome::default()
        };
        assert_eq!(summary(&outcome), "3 matches in 2 files");
        outcome.matches.truncate(1);
        assert_eq!(summary(&outcome), "1 match in 1 file");
        outcome.capped = true;
        assert!(summary(&outcome).contains("showing the first 2000"));
    }
}
